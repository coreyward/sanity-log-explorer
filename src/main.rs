use anyhow::{Context, Result};
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    prelude::{Constraint, CrosstermBackend, Frame, Layout, Rect, Terminal},
    style::{Color, Modifier, Style},
    text::{Line, Span},
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
    sample_url: String,
    request_count: u64,
    request_size_sum: u64,
    bandwidth_sum: u64,
}

impl PathStats {
    fn avg_request_size(&self) -> u64 {
        if self.request_count == 0 {
            0
        } else {
            self.bandwidth_sum / self.request_count
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

#[derive(Debug, Clone, Copy)]
enum RequestType {
    Image,
    File,
    Query,
    Other,
}

impl RequestType {
    fn label(self) -> char {
        match self {
            RequestType::Image => 'I',
            RequestType::File => 'F',
            RequestType::Query => 'Q',
            RequestType::Other => '?',
        }
    }

    fn color(self) -> Color {
        match self {
            RequestType::Image => Color::Green,
            RequestType::File => Color::Blue,
            RequestType::Query => Color::Yellow,
            RequestType::Other => Color::Gray,
        }
    }
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
        KeyCode::Enter => {
            if let Some(selected) = app.table_state.selected() {
                if let Some(item) = app.items.get(selected) {
                    let _ = open_url(&item.sample_url);
                }
            }
        }
        KeyCode::Char('r') => app.set_sort(SortField::Requests),
        KeyCode::Char('s') => app.set_sort(SortField::AvgRequestSize),
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
    let path_width = path_column_width(area.width);
    let header = Row::new([
        Cell::from("T"),
        header_cell("Path", 'p', app, SortField::Path),
        header_cell("Requests", 'r', app, SortField::Requests),
        header_cell("Size (Avg)", 's', app, SortField::AvgRequestSize),
        header_cell("Bandwidth", 'b', app, SortField::Bandwidth),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let visible_rows = visible_row_count(area.height);
    let (start, end) = visible_range(&app.items, app.table_state.selected(), visible_rows);
    let rows = app.items[start..end]
        .iter()
        .map(|item| row_for_item(item, path_width));

    let totals_row = totals_row(&app.items, path_width);
    let rows = rows.chain(std::iter::once(totals_row));

    let table = Table::new(
        rows,
        [
            Constraint::Length(2),
            Constraint::Length(path_width as u16),
            Constraint::Length(10),
            Constraint::Length(12),
            Constraint::Length(14),
        ],
    )
    .header(header)
    .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
    .block(
        Block::default()
            .title("Bandwidth by Path")
            .borders(Borders::ALL),
    );

    frame.render_stateful_widget(table, area, &mut app.table_state);
}

fn render_help(frame: &mut Frame, area: Rect) {
    let help = Block::default().title(
        "Keys: q quit | up/down or j/k move | enter open | p path | r requests | s avg size | b bandwidth | repeat toggles asc/desc",
    );
    frame.render_widget(help, area);
}

fn header_cell(label: &str, shortcut: char, app: &App, field: SortField) -> Cell<'static> {
    let line = header_line(label, shortcut, app, field);
    Cell::from(line)
}

fn header_line(label: &str, shortcut: char, app: &App, field: SortField) -> Line<'static> {
    let mut spans = Vec::new();
    let mut added_shortcut = false;
    for ch in label.chars() {
        if !added_shortcut && ch.eq_ignore_ascii_case(&shortcut) {
            spans.push(Span::styled(
                ch.to_string(),
                Style::default().add_modifier(Modifier::UNDERLINED),
            ));
            added_shortcut = true;
        } else {
            spans.push(Span::raw(ch.to_string()));
        }
    }

    if app.sort_field == field {
        spans.push(Span::raw(" "));
        spans.push(Span::raw(if app.descending { "↓" } else { "↑" }));
    }

    Line::from(spans)
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
        let path = if url.path().is_empty() {
            "/"
        } else {
            url.path()
        };

        let entry = map.entry(path.to_string()).or_insert_with(|| PathStats {
            path: path.to_string(),
            sample_url: url_str.to_string(),
            request_count: 0,
            request_size_sum: 0,
            bandwidth_sum: 0,
        });

        entry.request_count += 1;

        if let Some(req) = body.get("requestSize").and_then(as_u64) {
            entry.request_size_sum += req;
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

fn path_column_width(area_width: u16) -> usize {
    let fixed = 2u16 + 10 + 12 + 14;
    let spacing = 4u16;
    let borders = 2u16;
    let available = area_width.saturating_sub(fixed + spacing + borders);
    available.max(10) as usize
}

fn visible_row_count(height: u16) -> usize {
    let available = height.saturating_sub(3);
    let rows = available as usize;
    rows.max(1)
}

fn visible_range(
    items: &[PathStats],
    selected: Option<usize>,
    visible_rows: usize,
) -> (usize, usize) {
    if items.is_empty() {
        return (0, 0);
    }
    let max_items = visible_rows.saturating_sub(1);
    if max_items == 0 {
        return (0, 0);
    }
    let selected = selected.unwrap_or(0).min(items.len().saturating_sub(1));
    let mut start = 0usize;
    if selected >= max_items {
        start = selected + 1 - max_items;
    }
    let end = (start + max_items).min(items.len());
    (start, end)
}

fn row_for_item(item: &PathStats, path_width: usize) -> Row<'static> {
    let req_type = detect_request_type(&item.path);
    let stripped = strip_path(&item.path, req_type);
    let display_path = format_path_display(&stripped, path_width);
    let type_cell =
        Cell::from(req_type.label().to_string()).style(Style::default().fg(req_type.color()));

    Row::new([
        type_cell,
        Cell::from(display_path),
        Cell::from(item.request_count.to_string()),
        Cell::from(format_bytes(item.avg_request_size())),
        Cell::from(format_bytes(item.bandwidth_sum)),
    ])
}

fn totals_row(items: &[PathStats], path_width: usize) -> Row<'static> {
    let mut total_requests = 0u64;
    let mut total_bandwidth = 0u64;
    for item in items {
        total_requests += item.request_count;
        total_bandwidth += item.bandwidth_sum;
    }

    let avg_req = if total_requests == 0 {
        0
    } else {
        total_bandwidth / total_requests
    };
    let label = format_path_display("TOTAL", path_width);
    Row::new([
        Cell::from(""),
        Cell::from(label),
        Cell::from(total_requests.to_string()),
        Cell::from(format_bytes(avg_req)),
        Cell::from(format_bytes(total_bandwidth)),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD))
}

fn detect_request_type(path: &str) -> RequestType {
    if path.starts_with("/images/") {
        return RequestType::Image;
    }
    if path.starts_with("/files/") {
        return RequestType::File;
    }
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() >= 4 && parts[1] == "data" && parts[2] == "query" {
        return RequestType::Query;
    }
    RequestType::Other
}

fn strip_path(path: &str, kind: RequestType) -> String {
    match kind {
        RequestType::Image => strip_prefix_segments(path, 3).unwrap_or_else(|| path.to_string()),
        RequestType::File => strip_prefix_segments(path, 3).unwrap_or_else(|| path.to_string()),
        RequestType::Query => "query".to_string(),
        RequestType::Other => path.to_string(),
    }
}

fn strip_prefix_segments(path: &str, count: usize) -> Option<String> {
    let mut iter = path.split('/').filter(|s| !s.is_empty());
    for _ in 0..count {
        iter.next()?;
    }
    let remainder: Vec<&str> = iter.collect();
    if remainder.is_empty() {
        None
    } else {
        Some(remainder.join("/"))
    }
}

fn format_path_display(path: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if path.len() <= width {
        return path.to_string();
    }

    let (base, ext) = match path.rsplit_once('.') {
        Some((base, ext)) => (base, format!(".{ext}")),
        None => (path, String::new()),
    };

    if ext.is_empty() {
        return truncate_with_ellipsis(path, width);
    }

    if width <= ext.len() {
        return take_right(&ext, width);
    }

    let available = width - ext.len();
    if available <= 3 {
        let mut out = String::new();
        if available > 0 {
            out.push_str(&take_left(base, available.saturating_sub(1)));
        }
        out.push_str("...");
        out.push_str(&ext);
        return out.chars().take(width).collect();
    }

    let prefix_len = available - 3;
    let mut out = String::new();
    out.push_str(&take_left(base, prefix_len));
    out.push_str("...");
    out.push_str(&ext);
    out
}

fn truncate_with_ellipsis(value: &str, width: usize) -> String {
    if value.len() <= width {
        return value.to_string();
    }
    if width <= 3 {
        return take_left(value, width);
    }
    format!("{}...", take_left(value, width - 3))
}

fn take_left(value: &str, count: usize) -> String {
    value.chars().take(count).collect()
}

fn take_right(value: &str, count: usize) -> String {
    value
        .chars()
        .rev()
        .take(count)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
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

fn open_url(url: &str) -> Result<()> {
    if url.trim().is_empty() {
        return Ok(());
    }
    let mut cmd = if cfg!(target_os = "macos") {
        let mut cmd = std::process::Command::new("open");
        cmd.arg(url);
        cmd
    } else if cfg!(target_os = "windows") {
        let mut cmd = std::process::Command::new("cmd");
        cmd.args(["/C", "start", "", url]);
        cmd
    } else {
        let mut cmd = std::process::Command::new("xdg-open");
        cmd.arg(url);
        cmd
    };
    cmd.spawn().map(|_| ()).context("failed to open url")
}
