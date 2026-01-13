use anyhow::{Context, Result};
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    prelude::{Alignment, Constraint, CrosstermBackend, Frame, Layout, Rect, Terminal},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState, Tabs},
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortField {
    Path,
    Ext,
    Requests,
    AvgRequestSize,
    Bandwidth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Path,
    Type,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

#[derive(Debug, Clone)]
struct DisplayRow {
    label: String,
    ext: String,
    request_count: u64,
    bandwidth_sum: u64,
    req_type: RequestType,
    open_url: Option<String>,
    is_group: bool,
}

impl DisplayRow {
    fn avg_size(&self) -> u64 {
        if self.request_count == 0 {
            0
        } else {
            self.bandwidth_sum / self.request_count
        }
    }
}

struct App {
    base_items: Vec<PathStats>,
    items: Vec<DisplayRow>,
    sort_field: SortField,
    descending: bool,
    table_state: TableState,
    view_mode: ViewMode,
}

impl App {
    fn new(base_items: Vec<PathStats>) -> Self {
        let mut app = Self {
            base_items,
            items: Vec::new(),
            sort_field: SortField::Bandwidth,
            descending: true,
            table_state: TableState::default(),
            view_mode: ViewMode::Path,
        };
        app.rebuild_view();
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
            self.descending = !matches!(field, SortField::Path | SortField::Ext);
        }
        self.rebuild_view();
        self.clamp_selection();
    }

    fn toggle_view(&mut self) {
        self.view_mode = match self.view_mode {
            ViewMode::Path => ViewMode::Type,
            ViewMode::Type => ViewMode::Path,
        };
        self.rebuild_view();
        self.clamp_selection();
    }

    fn next_view(&mut self) {
        if self.view_mode == ViewMode::Path {
            self.toggle_view();
        }
    }

    fn previous_view(&mut self) {
        if self.view_mode == ViewMode::Type {
            self.toggle_view();
        }
    }

    fn rebuild_view(&mut self) {
        let descending = self.descending;
        let field = self.sort_field;
        self.items = build_display_rows(&self.base_items, self.view_mode, field, descending);
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
        KeyCode::Left | KeyCode::Char('h') => app.previous_view(),
        KeyCode::Right | KeyCode::Char('l') => app.next_view(),
        KeyCode::Tab => app.toggle_view(),
        KeyCode::Enter => {
            if let Some(selected) = app.table_state.selected() {
                if let Some(item) = app.items.get(selected) {
                    if let Some(url) = item.open_url.as_deref() {
                        let _ = open_url(url);
                    }
                }
            }
        }
        KeyCode::Char('r') => app.set_sort(SortField::Requests),
        KeyCode::Char('s') => app.set_sort(SortField::AvgRequestSize),
        KeyCode::Char('b') => app.set_sort(SortField::Bandwidth),
        KeyCode::Char('d') => app.set_sort(SortField::Path),
        KeyCode::Char('e') => app.set_sort(SortField::Ext),
        _ => {}
    }
    false
}

fn render(frame: &mut Frame, app: &mut App) {
    let chunks =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)])
            .split(frame.size());
    render_header(frame, chunks[0], app);
    render_table(frame, chunks[1], app);
    render_help(frame, chunks[2]);
}

fn render_header(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::horizontal([Constraint::Length(22), Constraint::Min(0)]).split(area);
    render_title(frame, chunks[0]);
    let right = Layout::horizontal([Constraint::Length(22), Constraint::Min(0)]).split(chunks[1]);
    render_tabs(frame, right[0], app);
    render_tabs_hint(frame, right[1]);
}

fn render_title(frame: &mut Frame, area: Rect) {
    let title = Paragraph::new("Sanity Log Explorer")
        .alignment(Alignment::Left)
        .style(Style::default().add_modifier(Modifier::BOLD));
    frame.render_widget(title, area);
}

fn render_tabs(frame: &mut Frame, area: Rect, app: &App) {
    let base_style = Style::default();
    let titles = ["By Asset", "By Type"]
        .iter()
        .map(|title| Line::from(Span::styled(*title, base_style)))
        .collect::<Vec<_>>();
    let selected = match app.view_mode {
        ViewMode::Path => 0,
        ViewMode::Type => 1,
    };
    let tabs = Tabs::new(titles)
        .select(selected)
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .divider(Span::raw(" "))
        .padding(" ", " ");
    frame.render_widget(tabs, area);
}

fn render_tabs_hint(frame: &mut Frame, area: Rect) {
    let hint = Paragraph::new("←→ switch tabs")
        .alignment(Alignment::Right)
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hint, area);
}
fn render_table(frame: &mut Frame, area: Rect, app: &mut App) {
    let id_width = id_column_width(area.width);
    let header = Row::new([
        type_header_cell(),
        header_cell("ID", 'd', app, SortField::Path),
        header_cell("Ext", 'e', app, SortField::Ext),
        header_cell_aligned("Requests", 'r', app, SortField::Requests, Alignment::Right),
        header_cell_aligned(
            "Size (Avg)",
            's',
            app,
            SortField::AvgRequestSize,
            Alignment::Right,
        ),
        header_cell_aligned(
            "Bandwidth",
            'b',
            app,
            SortField::Bandwidth,
            Alignment::Right,
        ),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let visible_rows = visible_row_count(area.height);
    let content_rows = visible_rows.saturating_sub(3);
    let (start, end) = visible_range(&app.items, app.table_state.selected(), content_rows);
    let rows = app.items[start..end]
        .iter()
        .map(|item| row_for_item(item, id_width));

    let divider_top = divider_row(id_width);
    let divider_bottom = divider_row(id_width);
    let totals_row = totals_row(&app.base_items, id_width);
    let rows = std::iter::once(divider_top)
        .chain(rows)
        .chain(std::iter::once(divider_bottom))
        .chain(std::iter::once(totals_row));

    let table = Table::new(
        rows,
        [
            Constraint::Length(2),
            Constraint::Length(id_width as u16),
            Constraint::Length(8),
            Constraint::Length(10),
            Constraint::Length(12),
            Constraint::Length(14),
        ],
    )
    .header(header)
    .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
    .block(Block::default().borders(Borders::ALL));

    let mut view_state = TableState::default();
    if let Some(selected) = app.table_state.selected() {
        if selected >= start && selected < end {
            view_state.select(Some(selected - start + 1));
        }
    }

    frame.render_stateful_widget(table, area, &mut view_state);
}

fn render_help(frame: &mut Frame, area: Rect) {
    let help = Block::default().title(
        "Keys: q quit | up/down or j/k move | left/right or h/l tabs | enter open | tab view | d id | e ext | r requests | s avg size | b bandwidth | repeat toggles asc/desc",
    );
    frame.render_widget(help, area);
}

fn type_header_cell() -> Cell<'static> {
    let line = Line::from(vec![Span::raw("T")]);
    Cell::from(line)
}

fn header_cell(label: &str, shortcut: char, app: &App, field: SortField) -> Cell<'static> {
    let line = header_line(label, shortcut, app, field);
    Cell::from(line)
}

fn header_cell_aligned(
    label: &str,
    shortcut: char,
    app: &App,
    field: SortField,
    alignment: Alignment,
) -> Cell<'static> {
    let line = header_line(label, shortcut, app, field);
    let text = Text::from(line).alignment(alignment);
    Cell::from(text)
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

fn id_column_width(area_width: u16) -> usize {
    let fixed = 2u16 + 8 + 10 + 12 + 14;
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
    items: &[DisplayRow],
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

fn build_display_rows(
    base_items: &[PathStats],
    view_mode: ViewMode,
    field: SortField,
    descending: bool,
) -> Vec<DisplayRow> {
    match view_mode {
        ViewMode::Path => {
            let mut rows: Vec<DisplayRow> = base_items
                .iter()
                .map(|item| {
                    let req_type = detect_request_type(&item.path);
                    let (id, ext) = asset_id_and_ext(&item.path, req_type);
                    DisplayRow {
                        label: id,
                        ext,
                        request_count: item.request_count,
                        bandwidth_sum: item.bandwidth_sum,
                        req_type,
                        open_url: Some(item.sample_url.clone()),
                        is_group: false,
                    }
                })
                .collect();
            sort_display_rows(&mut rows, field, descending);
            rows
        }
        ViewMode::Type => build_type_rows(base_items, field, descending),
    }
}

#[derive(Default)]
struct Agg {
    request_count: u64,
    bandwidth_sum: u64,
    sample_url: Option<String>,
}

fn build_type_rows(
    base_items: &[PathStats],
    field: SortField,
    descending: bool,
) -> Vec<DisplayRow> {
    let mut type_map: HashMap<RequestType, Agg> = HashMap::new();
    let mut ext_map: HashMap<(RequestType, String), Agg> = HashMap::new();

    for item in base_items {
        let req_type = detect_request_type(&item.path);
        let type_entry = type_map.entry(req_type).or_default();
        type_entry.request_count += item.request_count;
        type_entry.bandwidth_sum += item.bandwidth_sum;
        if type_entry.sample_url.is_none() {
            type_entry.sample_url = Some(item.sample_url.clone());
        }

        if matches!(req_type, RequestType::Image | RequestType::File) {
            let ext = extract_extension(&item.path).unwrap_or_else(|| "no ext".to_string());
            let ext_entry = ext_map.entry((req_type, ext)).or_default();
            ext_entry.request_count += item.request_count;
            ext_entry.bandwidth_sum += item.bandwidth_sum;
            if ext_entry.sample_url.is_none() {
                ext_entry.sample_url = Some(item.sample_url.clone());
            }
        }
    }

    let mut type_rows: Vec<DisplayRow> = Vec::new();
    for req_type in [
        RequestType::Image,
        RequestType::File,
        RequestType::Query,
        RequestType::Other,
    ] {
        let agg = match type_map.get(&req_type) {
            Some(agg) => agg,
            None => continue,
        };
        type_rows.push(DisplayRow {
            label: type_label(req_type).to_string(),
            ext: String::new(),
            request_count: agg.request_count,
            bandwidth_sum: agg.bandwidth_sum,
            req_type,
            open_url: None,
            is_group: true,
        });
    }

    sort_display_rows(&mut type_rows, field, descending);

    let mut rows: Vec<DisplayRow> = Vec::new();
    for type_row in type_rows {
        let req_type = type_row.req_type;
        rows.push(type_row);
        if matches!(req_type, RequestType::Image | RequestType::File) {
            let mut ext_rows: Vec<DisplayRow> = ext_map
                .iter()
                .filter_map(|((kind, ext), agg)| {
                    if *kind != req_type {
                        return None;
                    }
                    let label = if ext == "no ext" {
                        "  (no ext)".to_string()
                    } else {
                        "  ".to_string()
                    };
                    Some(DisplayRow {
                        label,
                        ext: if ext == "no ext" {
                            "(none)".to_string()
                        } else {
                            format!(".{ext}")
                        },
                        request_count: agg.request_count,
                        bandwidth_sum: agg.bandwidth_sum,
                        req_type,
                        open_url: agg.sample_url.clone(),
                        is_group: false,
                    })
                })
                .collect();
            sort_display_rows(&mut ext_rows, field, descending);
            rows.extend(ext_rows);
        }
    }

    rows
}

fn sort_display_rows(rows: &mut [DisplayRow], field: SortField, descending: bool) {
    rows.sort_by(|a, b| {
        let ordering = match field {
            SortField::Path => {
                let a_rank = if a.req_type == RequestType::Query {
                    0
                } else {
                    1
                };
                let b_rank = if b.req_type == RequestType::Query {
                    0
                } else {
                    1
                };
                (a_rank, &a.label).cmp(&(b_rank, &b.label))
            }
            SortField::Ext => a.ext.cmp(&b.ext),
            SortField::Requests => a.request_count.cmp(&b.request_count),
            SortField::AvgRequestSize => a.avg_size().cmp(&b.avg_size()),
            SortField::Bandwidth => a.bandwidth_sum.cmp(&b.bandwidth_sum),
        };
        if descending {
            ordering.reverse()
        } else {
            ordering
        }
    });
}

fn type_label(kind: RequestType) -> &'static str {
    match kind {
        RequestType::Image => "Images",
        RequestType::File => "Files",
        RequestType::Query => "GROQ Queries",
        RequestType::Other => "Other",
    }
}

fn row_for_item(item: &DisplayRow, path_width: usize) -> Row<'static> {
    let display_path = format_id_display(&item.label, path_width);
    let type_cell = Cell::from(item.req_type.label().to_string())
        .style(Style::default().fg(item.req_type.color()));
    let row_style = if item.is_group {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };

    Row::new([
        type_cell,
        Cell::from(display_path),
        Cell::from(item.ext.clone()),
        right_cell(format_count(item.request_count)),
        right_cell(format_bytes(item.avg_size())),
        right_cell(format_bytes(item.bandwidth_sum)),
    ])
    .style(row_style)
}

fn divider_row(id_width: usize) -> Row<'static> {
    let fill = |width: usize| "─".repeat(width.max(1));
    Row::new([
        Cell::from(fill(2)),
        Cell::from(fill(id_width)),
        Cell::from(fill(8)),
        Cell::from(fill(10)),
        Cell::from(fill(12)),
        Cell::from(fill(14)),
    ])
    .style(Style::default().fg(Color::DarkGray))
}

fn right_cell(value: String) -> Cell<'static> {
    Cell::from(Text::from(value).alignment(Alignment::Right))
}

fn totals_row(items: &[PathStats], id_width: usize) -> Row<'static> {
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
    let label = format_id_display("TOTAL", id_width);
    Row::new([
        Cell::from(""),
        Cell::from(label),
        Cell::from(""),
        right_cell(format_count(total_requests)),
        right_cell(format_bytes(avg_req)),
        right_cell(format_bytes(total_bandwidth)),
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

fn asset_id_and_ext(path: &str, kind: RequestType) -> (String, String) {
    match kind {
        RequestType::Image => {
            let remainder = strip_prefix_segments(path, 3).unwrap_or_else(|| path.to_string());
            let file = remainder.split('/').last().unwrap_or(remainder.as_str());
            let (name, ext) = match file.rsplit_once('.') {
                Some((name, ext)) => (name, ext.to_string()),
                None => (file, String::new()),
            };
            let id = name.split('-').next().unwrap_or(name).to_string();
            (id, format_ext(&ext))
        }
        RequestType::File => {
            let remainder = strip_prefix_segments(path, 3).unwrap_or_else(|| path.to_string());
            let file = remainder.split('/').last().unwrap_or(remainder.as_str());
            let (name, ext) = match file.rsplit_once('.') {
                Some((name, ext)) => (name.to_string(), ext.to_string()),
                None => (file.to_string(), String::new()),
            };
            (name, format_ext(&ext))
        }
        RequestType::Query => ("GROQ Queries".to_string(), String::new()),
        RequestType::Other => {
            let remainder = strip_prefix_segments(path, 0).unwrap_or_else(|| path.to_string());
            let ext = extract_extension(&remainder).unwrap_or_default();
            (remainder, format_ext(&ext))
        }
    }
}

fn format_ext(ext: &str) -> String {
    if ext.is_empty() {
        String::new()
    } else {
        format!(".{}", ext)
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

fn extract_extension(path: &str) -> Option<String> {
    let (_, ext) = path.rsplit_once('.')?;
    if ext.is_empty() {
        None
    } else {
        Some(ext.to_lowercase())
    }
}

fn format_id_display(value: &str, width: usize) -> String {
    truncate_with_ellipsis(value, width)
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

fn format_count(value: u64) -> String {
    if value >= 1_000_000 {
        return format!("{:.1}M", value as f64 / 1_000_000.0);
    }
    if value >= 1_000 {
        return format!("{:.1}K", value as f64 / 1_000.0);
    }
    value.to_string()
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
