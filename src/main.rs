use anyhow::{Context, Result};
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    prelude::{Constraint, CrosstermBackend, Frame, Layout, Rect, Terminal},
    style::{Modifier, Style},
    widgets::{Block, Borders, Cell, Row, Table, TableState},
};
use serde_json::Value;
use std::{
    collections::HashMap,
    env,
    fs::File,
    io::{self, BufRead, BufReader, Stderr},
    time::Duration,
};
use url::Url;

#[derive(Debug, Clone)]
struct PathStats {
    path: String,
    request_count: u64,
    request_size_sum: u64,
    request_size_count: u64,
    bandwidth_sum: u64,
}

impl PathStats {
    fn avg_request_size(&self) -> u64 {
        if self.request_size_count == 0 {
            0
        } else {
            self.request_size_sum / self.request_size_count
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortField {
    Path,
    Requests,
    AvgRequestSize,
    Bandwidth,
}

struct App {
    items: Vec<PathStats>,
    sort_field: SortField,
    descending: bool,
    table_state: TableState,
}

impl App {
    fn new(items: Vec<PathStats>) -> Self {
        let mut app = Self {
            items,
            sort_field: SortField::Bandwidth,
            descending: true,
            table_state: TableState::default(),
        };
        app.sort_items();
        if !app.items.is_empty() {
            app.table_state.select(Some(0));
        }
        app
    }

    fn set_sort(&mut self, field: SortField) {
        if self.sort_field == field {
            self.descending = !self.descending;
        } else {
            self.sort_field = field;
            self.descending = field != SortField::Path;
        }
        self.sort_items();
        self.clamp_selection();
    }

    fn sort_items(&mut self) {
        let descending = self.descending;
        let field = self.sort_field;
        self.items.sort_by(|a, b| {
            let ordering = match field {
                SortField::Path => a.path.cmp(&b.path),
                SortField::Requests => a.request_count.cmp(&b.request_count),
                SortField::AvgRequestSize => a.avg_request_size().cmp(&b.avg_request_size()),
                SortField::Bandwidth => a.bandwidth_sum.cmp(&b.bandwidth_sum),
            };
            if descending {
                ordering.reverse()
            } else {
                ordering
            }
        });
    }

    fn clamp_selection(&mut self) {
        let len = self.items.len();
        let next = match self.table_state.selected() {
            Some(idx) if idx < len => idx,
            _ if len == 0 => {
                self.table_state.select(None);
                return;
            }
            _ => len.saturating_sub(1),
        };
        self.table_state.select(Some(next));
    }

    fn next(&mut self) {
        if self.items.is_empty() {
            return;
        }
        let next = match self.table_state.selected() {
            Some(idx) if idx + 1 < self.items.len() => idx + 1,
            _ => self.items.len() - 1,
        };
        self.table_state.select(Some(next));
    }

    fn previous(&mut self) {
        if self.items.is_empty() {
            return;
        }
        let prev = match self.table_state.selected() {
            Some(idx) if idx > 0 => idx - 1,
            _ => 0,
        };
        self.table_state.select(Some(prev));
    }
}

fn main() -> Result<()> {
    let path = env::args().nth(1).unwrap_or_default();
    if path.is_empty() {
        eprintln!("Usage: bandwidth_tui <ndjson-file>");
        return Ok(());
    }

    let stats = load_stats(&path).with_context(|| format!("failed to load {path}"))?;
    let mut terminal = setup_terminal()?;

    let result = run_app(&mut terminal, stats);

    restore_terminal(&mut terminal)?;
    result
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stderr>>> {
    enable_raw_mode()?;
    let mut stderr = io::stderr();
    execute!(stderr, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stderr);
    Ok(Terminal::new(backend)?)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stderr>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn run_app(terminal: &mut Terminal<CrosstermBackend<Stderr>>, items: Vec<PathStats>) -> Result<()> {
    let mut app = App::new(items);
    loop {
        terminal.draw(|frame| render(frame, &mut app))?;

        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                if handle_key(&mut app, key) {
                    break;
                }
            }
        }
    }
    Ok(())
}

fn handle_key(app: &mut App, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => return true,
        KeyCode::Up | KeyCode::Char('k') => app.previous(),
        KeyCode::Down | KeyCode::Char('j') => app.next(),
        KeyCode::Char('r') => app.set_sort(SortField::Requests),
        KeyCode::Char('a') => app.set_sort(SortField::AvgRequestSize),
        KeyCode::Char('b') => app.set_sort(SortField::Bandwidth),
        KeyCode::Char('p') => app.set_sort(SortField::Path),
        _ => {}
    }
    false
}

fn render(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(frame.size());
    render_table(frame, chunks[0], app);
    render_help(frame, chunks[1]);
}

fn render_table(frame: &mut Frame, area: Rect, app: &mut App) {
    let header = Row::new([
        header_cell("Path", app, SortField::Path),
        header_cell("Requests", app, SortField::Requests),
        header_cell("Avg Req", app, SortField::AvgRequestSize),
        header_cell("Bandwidth", app, SortField::Bandwidth),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let rows = app.items.iter().map(|item| {
        Row::new([
            Cell::from(item.path.clone()),
            Cell::from(item.request_count.to_string()),
            Cell::from(format_bytes(item.avg_request_size())),
            Cell::from(format_bytes(item.bandwidth_sum)),
        ])
    });

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(58),
            Constraint::Length(10),
            Constraint::Length(12),
            Constraint::Length(14),
        ],
    )
    .header(header)
    .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
    .block(Block::default().title("Bandwidth by Path").borders(Borders::ALL));

    frame.render_stateful_widget(table, area, &mut app.table_state);
}

fn render_help(frame: &mut Frame, area: Rect) {
    let help = Block::default().title("Keys: q quit | up/down or j/k move | p path | r requests | a avg req | b bandwidth | repeat toggles asc/desc");
    frame.render_widget(help, area);
}

fn header_cell(label: &str, app: &App, field: SortField) -> Cell<'static> {
    let mut text = label.to_string();
    if app.sort_field == field {
        text.push(' ');
        text.push(if app.descending { 'v' } else { '^' });
    }
    Cell::from(text)
}

fn load_stats(path: &str) -> Result<Vec<PathStats>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut map: HashMap<String, PathStats> = HashMap::new();

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => continue,
        };

        let body = match value.get("body") {
            Some(Value::Object(map)) => map,
            _ => continue,
        };

        let url_str = match body.get("url").and_then(|v| v.as_str()) {
            Some(url) => url,
            None => continue,
        };

        let url = match Url::parse(url_str) {
            Ok(url) => url,
            Err(_) => continue,
        };
        let path = if url.path().is_empty() { "/" } else { url.path() };

        let entry = map
            .entry(path.to_string())
            .or_insert_with(|| PathStats {
                path: path.to_string(),
                request_count: 0,
                request_size_sum: 0,
                request_size_count: 0,
                bandwidth_sum: 0,
            });

        entry.request_count += 1;

        if let Some(req) = body.get("requestSize").and_then(as_u64) {
            entry.request_size_sum += req;
            entry.request_size_count += 1;
        }

        if let Some(resp) = body.get("responseSize").and_then(as_u64) {
            entry.bandwidth_sum += resp;
        }
    }

    let mut stats: Vec<PathStats> = map.into_values().collect();
    stats.sort_by(|a, b| b.bandwidth_sum.cmp(&a.bandwidth_sum));
    Ok(stats)
}

fn as_u64(value: &Value) -> Option<u64> {
    match value {
        Value::Number(num) => num.as_u64(),
        Value::String(s) => s.parse::<u64>().ok(),
        _ => None,
    }
}

fn format_bytes(value: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = value as f64;
    let mut unit = 0usize;
    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", value, UNITS[unit])
    } else {
        format!("{:.2} {}", size, UNITS[unit])
    }
}
