//! Nebula responsive terminal client.
//!
//! * Terminal >= 100 columns → three panes (conversations/moments | chat/detail
//!   | identity + telemetry + CAS).
//! * Terminal < 100 columns → single pane with `F1/F2/F3` panel switching.
//!
//! Keyboard: `↑/↓` select, `i` compose, `p` send an image path, `v` send a
//! synthesized test voice note, `n` publish a moment, `l` like, `c` comment,
//! `PgUp/PgDn` scroll, `q` quit (broadcasts LEAVE).

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use nebula_core::{guess_mime, MessageRecord, MomentView, NebulaNode, PeerRecord, SendMessageRequest};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use serde_json::{json, Value};
use std::io::stdout;
use std::sync::Arc;
use std::time::{Duration, Instant};

const ACCENT: Color = Color::Rgb(99, 102, 241);
const ACCENT2: Color = Color::Rgb(6, 182, 212);
const MUTED: Color = Color::Rgb(148, 163, 184);
const FAINT: Color = Color::Rgb(100, 116, 139);
const ONLINE: Color = Color::Rgb(16, 185, 129);
const QUEUED: Color = Color::Rgb(245, 158, 11);
const DANGER: Color = Color::Rgb(244, 63, 94);

const SPARK: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Chat,
    Moments,
    Radar,
}

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Tab::Chat => "1 星讯",
            Tab::Moments => "2 星云圈",
            Tab::Radar => "3 节点雷达",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputKind {
    Chat,
    Moment,
    Comment,
    ImagePath,
}

struct InputState {
    active: bool,
    kind: InputKind,
    buffer: String,
}

struct ConvRow {
    id: String,
    label: String,
    status: String,
    preview: String,
}

pub struct TuiApp {
    node: Arc<NebulaNode>,
    tab: Tab,
    convs: Vec<ConvRow>,
    selected_conv: String,
    messages: Vec<MessageRecord>,
    msg_scroll: u16,
    auto_scroll: bool,
    moments: Vec<MomentView>,
    selected_moment: Option<String>,
    peers: Vec<PeerRecord>,
    selected_peer: Option<String>,
    blob_count: i64,
    blob_bytes: i64,
    pending_blobs: usize,
    input: InputState,
    status: String,
    should_quit: bool,
}

impl TuiApp {
    pub fn new(node: Arc<NebulaNode>) -> Self {
        Self {
            node,
            tab: Tab::Chat,
            convs: Vec::new(),
            selected_conv: "lobby".to_string(),
            messages: Vec::new(),
            msg_scroll: 0,
            auto_scroll: true,
            moments: Vec::new(),
            selected_moment: None,
            peers: Vec::new(),
            selected_peer: None,
            blob_count: 0,
            blob_bytes: 0,
            pending_blobs: 0,
            input: InputState {
                active: false,
                kind: InputKind::Chat,
                buffer: String::new(),
            },
            status: "Nebula TUI 已就绪 — 按 i 开始输入".to_string(),
            should_quit: false,
        }
    }

    // ------------------------------------------------------------------
    // State
    // ------------------------------------------------------------------
    fn refresh(&mut self) {
        if let Ok(convs) = self.node.conversations() {
            self.convs = convs
                .into_iter()
                .map(|conv| {
                    let (label, status) = if conv.conversation_id == "lobby" {
                        ("◆ 星云大厅".to_string(), "公共频道".to_string())
                    } else {
                        (
                            format!(
                                "● {}",
                                conv.peer_name
                                    .clone()
                                    .unwrap_or_else(|| short_id(conv.conversation_id.as_str()))
                            ),
                            conv.peer_status.clone().unwrap_or_else(|| "offline".to_string()),
                        )
                    };
                    let preview = conv
                        .last_message
                        .as_ref()
                        .map(message_preview)
                        .unwrap_or_else(|| "还没有消息".to_string());
                    ConvRow {
                        id: conv.conversation_id,
                        label,
                        status,
                        preview,
                    }
                })
                .collect();
        }
        if !self.convs.iter().any(|c| c.id == self.selected_conv) {
            self.selected_conv = self
                .convs
                .first()
                .map(|c| c.id.clone())
                .unwrap_or_else(|| "lobby".to_string());
            self.auto_scroll = true;
        }

        let messages = self
            .node
            .db
            .list_messages(&self.selected_conv, 300)
            .unwrap_or_default();
        let last_changed = messages.last().map(|m| m.msg_id.clone())
            != self.messages.last().map(|m| m.msg_id.clone());
        if last_changed || messages.len() != self.messages.len() {
            self.auto_scroll = true;
        }
        self.messages = messages;

        if let Ok(moments) = self.node.moments_view() {
            if self
                .selected_moment
                .as_ref()
                .map(|id| !moments.iter().any(|m| &m.moment_id == id))
                .unwrap_or(true)
            {
                self.selected_moment = moments.first().map(|m| m.moment_id.clone());
            }
            self.moments = moments;
        }

        if let Ok(peers) = self.node.db.list_peers() {
            if self
                .selected_peer
                .as_ref()
                .map(|id| !peers.iter().any(|p| &p.peer_id == id))
                .unwrap_or(true)
            {
                self.selected_peer = peers.first().map(|p| p.peer_id.clone());
            }
            self.peers = peers;
        }

        if let Ok(stats) = self.node.db.blob_stats() {
            self.blob_count = stats.count;
            self.blob_bytes = stats.bytes;
        }
        self.pending_blobs = self.node.db.pending_blobs(200).map(|v| v.len()).unwrap_or(0);
    }

    fn reload_messages(&mut self) {
        self.messages = self
            .node
            .db
            .list_messages(&self.selected_conv, 300)
            .unwrap_or_default();
        self.auto_scroll = true;
        self.msg_scroll = 0;
    }

    fn set_status(&mut self, message: impl Into<String>) {
        self.status = message.into();
    }

    fn selected_moment(&self) -> Option<&MomentView> {
        let id = self.selected_moment.as_ref()?;
        self.moments.iter().find(|m| &m.moment_id == id)
    }

    fn selected_peer(&self) -> Option<&PeerRecord> {
        let id = self.selected_peer.as_ref()?;
        self.peers.iter().find(|p| &p.peer_id == id)
    }

    fn online_peers(&self) -> usize {
        self.peers.iter().filter(|p| p.status == "online").count()
    }

    fn move_selection(&mut self, delta: isize) {
        match self.tab {
            Tab::Chat => {
                if self.convs.is_empty() {
                    return;
                }
                let index = self
                    .convs
                    .iter()
                    .position(|c| c.id == self.selected_conv)
                    .unwrap_or(0);
                let next = (index as isize + delta)
                    .clamp(0, self.convs.len() as isize - 1) as usize;
                if self.convs[next].id != self.selected_conv {
                    self.selected_conv = self.convs[next].id.clone();
                    self.reload_messages();
                }
            }
            Tab::Moments => {
                if self.moments.is_empty() {
                    return;
                }
                let index = self
                    .moments
                    .iter()
                    .position(|m| Some(&m.moment_id) == self.selected_moment.as_ref())
                    .unwrap_or(0);
                let next = (index as isize + delta)
                    .clamp(0, self.moments.len() as isize - 1) as usize;
                self.selected_moment = Some(self.moments[next].moment_id.clone());
            }
            Tab::Radar => {
                if self.peers.is_empty() {
                    return;
                }
                let index = self
                    .peers
                    .iter()
                    .position(|p| Some(&p.peer_id) == self.selected_peer.as_ref())
                    .unwrap_or(0);
                let next = (index as isize + delta)
                    .clamp(0, self.peers.len() as isize - 1) as usize;
                self.selected_peer = Some(self.peers[next].peer_id.clone());
            }
        }
    }

    // ------------------------------------------------------------------
    // Input & actions
    // ------------------------------------------------------------------
    fn begin_input(&mut self, kind: InputKind) {
        self.input.active = true;
        self.input.kind = kind;
        self.input.buffer.clear();
    }

    async fn submit_input(&mut self) -> Result<()> {
        let kind = self.input.kind;
        let text = self.input.buffer.trim().to_string();
        self.input.active = false;
        self.input.buffer.clear();
        if text.is_empty() {
            self.set_status("已取消输入");
            return Ok(());
        }
        match kind {
            InputKind::Chat => {
                let conv = self.selected_conv.clone();
                let request = SendMessageRequest {
                    conversation_id: Some(conv),
                    msg_type: Some("text".to_string()),
                    content: Some(text),
                    ..Default::default()
                };
                match self.node.send_message(request).await {
                    Ok(message) => {
                        self.auto_scroll = true;
                        self.set_status(format!("已发送 ({})", delivery_label(&message.delivery_status)));
                    }
                    Err(err) => self.set_status(format!("发送失败: {err:#}")),
                }
            }
            InputKind::Moment => match self.node.create_moment_post(&text, Vec::new(), None, None) {
                Ok(_) => self.set_status("星云动态已发布，正在 Gossip 传染"),
                Err(err) => self.set_status(format!("发布失败: {err:#}")),
            },
            InputKind::Comment => {
                let moment_id = self.selected_moment.clone();
                match moment_id {
                    Some(id) => match self.node.create_moment_interaction(&id, "COMMENT", &text) {
                        Ok(_) => self.set_status("评论已发布"),
                        Err(err) => self.set_status(format!("评论失败: {err:#}")),
                    },
                    None => self.set_status("当前没有可评论的动态"),
                }
            }
            InputKind::ImagePath => self.send_image_path(&text).await,
        }
        self.refresh();
        Ok(())
    }

    async fn send_image_path(&mut self, path: &str) {
        let bytes = match tokio::fs::read(path).await {
            Ok(bytes) => bytes,
            Err(err) => {
                self.set_status(format!("读取图片失败: {err}"));
                return;
            }
        };
        let mime = guess_mime(path);
        let (blob_id, size) = match self.node.store_blob(&bytes, mime) {
            Ok(result) => result,
            Err(err) => {
                self.set_status(format!("写入 CAS 失败: {err:#}"));
                return;
            }
        };
        let file_name = std::path::Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("image")
            .to_string();
        let request = SendMessageRequest {
            conversation_id: Some(self.selected_conv.clone()),
            msg_type: Some("image".to_string()),
            content: Some(file_name),
            blob_id: Some(blob_id),
            media_meta: Some(json!({
                "mime_type": mime,
                "size_bytes": size,
                "width": 0,
                "height": 0
            })),
            ..Default::default()
        };
        match self.node.send_message(request).await {
            Ok(message) => {
                self.auto_scroll = true;
                self.set_status(format!(
                    "图片已通过 CAS 发送 ({}, {})",
                    fmt_bytes(size as i64),
                    delivery_label(&message.delivery_status)
                ));
            }
            Err(err) => self.set_status(format!("图片发送失败: {err:#}")),
        }
    }

    async fn send_voice(&mut self) {
        let (wav, waveform, duration) = synth_voice();
        let (blob_id, size) = match self.node.store_blob(&wav, "audio/wav") {
            Ok(result) => result,
            Err(err) => {
                self.set_status(format!("写入语音 CAS 失败: {err:#}"));
                return;
            }
        };
        let request = SendMessageRequest {
            conversation_id: Some(self.selected_conv.clone()),
            msg_type: Some("voice".to_string()),
            content: Some("TUI 合成测试语音".to_string()),
            blob_id: Some(blob_id),
            media_meta: Some(json!({
                "duration_ms": duration,
                "waveform": waveform,
                "mime_type": "audio/wav",
                "size_bytes": size
            })),
            ..Default::default()
        };
        match self.node.send_message(request).await {
            Ok(message) => {
                self.auto_scroll = true;
                self.set_status(format!(
                    "语音已发送 ({}s, {})",
                    duration as f64 / 1000.0,
                    delivery_label(&message.delivery_status)
                ));
            }
            Err(err) => self.set_status(format!("语音发送失败: {err:#}")),
        }
    }

    async fn like_selected(&mut self) {
        let id = self.selected_moment.clone();
        match id {
            Some(id) => match self.node.create_moment_interaction(&id, "LIKE", "") {
                Ok(_) => self.set_status("已点赞 💖"),
                Err(err) => self.set_status(format!("点赞失败: {err:#}")),
            },
            None => self.set_status("当前没有可点赞的动态"),
        }
        self.refresh();
    }

    async fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        if key.kind == KeyEventKind::Release {
            return Ok(());
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c'))
        {
            self.should_quit = true;
            return Ok(());
        }
        if self.input.active {
            match key.code {
                KeyCode::Esc => {
                    self.input.active = false;
                    self.input.buffer.clear();
                    self.set_status("已取消输入");
                }
                KeyCode::Enter => self.submit_input().await?,
                KeyCode::Backspace => {
                    self.input.buffer.pop();
                }
                KeyCode::Char(ch) => self.input.buffer.push(ch),
                _ => {}
            }
            return Ok(());
        }

        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('1') | KeyCode::F(1) => self.tab = Tab::Chat,
            KeyCode::Char('2') | KeyCode::F(2) => self.tab = Tab::Moments,
            KeyCode::Char('3') | KeyCode::F(3) => self.tab = Tab::Radar,
            KeyCode::Up => self.move_selection(-1),
            KeyCode::Down => self.move_selection(1),
            KeyCode::PageUp => {
                self.auto_scroll = false;
                self.msg_scroll = self.msg_scroll.saturating_sub(5);
            }
            KeyCode::PageDown => {
                self.msg_scroll = self.msg_scroll.saturating_add(5);
            }
            KeyCode::End => self.auto_scroll = true,
            KeyCode::Char('i') => match self.tab {
                Tab::Chat => self.begin_input(InputKind::Chat),
                Tab::Moments => self.begin_input(InputKind::Moment),
                Tab::Radar => self.set_status("雷达面板无需输入"),
            },
            KeyCode::Char('p') => {
                if self.tab == Tab::Chat {
                    self.begin_input(InputKind::ImagePath);
                } else {
                    self.set_status("请在星讯面板中发送图片");
                }
            }
            KeyCode::Char('v') => {
                if self.tab == Tab::Chat {
                    self.send_voice().await;
                } else {
                    self.set_status("请在星讯面板中发送语音");
                }
            }
            KeyCode::Char('n') => {
                self.tab = Tab::Moments;
                self.begin_input(InputKind::Moment);
            }
            KeyCode::Char('l') => {
                if self.tab == Tab::Moments {
                    self.like_selected().await;
                } else {
                    self.set_status("请切换到星云圈后再点赞");
                }
            }
            KeyCode::Char('c') => {
                if self.tab == Tab::Moments {
                    if self.selected_moment().is_some() {
                        self.begin_input(InputKind::Comment);
                    } else {
                        self.set_status("当前没有可评论的动态");
                    }
                } else {
                    self.set_status("请切换到星云圈后再评论");
                }
            }
            KeyCode::Esc => {
                self.auto_scroll = true;
                self.set_status("已回到最新消息");
            }
            _ => {}
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Rendering
    // ------------------------------------------------------------------
    fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let wide = area.width >= 100;
        let chunks = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(area);

        self.draw_header(frame, chunks[0], wide);

        if wide {
            let cols = Layout::horizontal([
                Constraint::Percentage(25),
                Constraint::Percentage(50),
                Constraint::Percentage(25),
            ])
            .split(chunks[1]);
            self.draw_list_panel(frame, cols[0]);
            self.draw_detail_panel(frame, cols[1]);
            self.draw_telemetry(frame, cols[2]);
        } else {
            let rows = Layout::vertical([Constraint::Percentage(35), Constraint::Percentage(65)])
                .split(chunks[1]);
            self.draw_list_panel(frame, rows[0]);
            self.draw_detail_panel(frame, rows[1]);
        }

        self.draw_footer(frame, chunks[2], wide);
    }

    fn draw_header(&self, frame: &mut Frame, area: Rect, wide: bool) {
        let identity = self.node.identity();
        let mut line1 = vec![
            Span::styled(
                " NEBULA 星云 ",
                Style::default()
                    .fg(Color::White)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                identity.nickname.clone(),
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  {}", short_id(&identity.peer_id)), Style::default().fg(MUTED)),
            Span::styled(
                format!("  p2p:{}", self.node.p2p_port),
                Style::default().fg(FAINT),
            ),
            Span::styled(
                format!("  在线 {}  ", self.online_peers()),
                Style::default().fg(ONLINE),
            ),
            Span::styled(
                format!("CAS {} · {}", self.blob_count, fmt_bytes(self.blob_bytes)),
                Style::default().fg(ACCENT2),
            ),
            Span::styled(
                format!("  待拉取 {}", self.pending_blobs),
                Style::default().fg(if self.pending_blobs > 0 { QUEUED } else { FAINT }),
            ),
        ];
        if !wide {
            line1.push(Span::styled(
                format!("  {}x{}", area.width, area.height),
                Style::default().fg(FAINT),
            ));
        }

        let tabs = [Tab::Chat, Tab::Moments, Tab::Radar];
        let mut line2 = vec![Span::raw(" ")];
        for tab in tabs {
            let style = if tab == self.tab {
                Style::default()
                    .fg(Color::White)
                    .bg(ACCENT2)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(MUTED)
            };
            line2.push(Span::styled(format!(" {} ", tab.label()), style));
            line2.push(Span::raw(" "));
        }
        line2.push(Span::styled(
            if wide {
                "↑/↓ 选择   i 输入   p 图片   v 语音   n 动态   l 点赞   c 评论   q 退出"
            } else {
                "F1/F2/F3 切换面板   ↑/↓ 选择   i 输入   q 退出"
            },
            Style::default().fg(FAINT),
        ));

        let paragraph = Paragraph::new(Text::from(vec![Line::from(line1), Line::from(line2)]));
        frame.render_widget(paragraph, area);
    }

    fn draw_list_panel(&self, frame: &mut Frame, area: Rect) {
        let title = match self.tab {
            Tab::Chat => " 会话与节点 ",
            Tab::Moments => " 星云圈动态 ",
            Tab::Radar => " 节点雷达 ",
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(ACCENT))
            .title(Span::styled(
                title,
                Style::default().fg(ACCENT2).add_modifier(Modifier::BOLD),
            ));
        let mut lines: Vec<Line> = Vec::new();
        let selected_index;

        match self.tab {
            Tab::Chat => {
                selected_index = self
                    .convs
                    .iter()
                    .position(|c| c.id == self.selected_conv)
                    .unwrap_or(0);
                if self.convs.is_empty() {
                    lines.push(Line::from(Span::styled(
                        " 尚未发现节点",
                        Style::default().fg(FAINT),
                    )));
                }
                for (index, conv) in self.convs.iter().enumerate() {
                    let selected = index == selected_index;
                    let online = conv.status == "online" || conv.id == "lobby";
                    let marker = if selected { "›" } else { " " };
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("{marker} "),
                            Style::default().fg(if selected { ACCENT2 } else { FAINT }),
                        ),
                        Span::styled(
                            conv.label.clone(),
                            if selected {
                                Style::default()
                                    .fg(Color::White)
                                    .add_modifier(Modifier::BOLD)
                            } else {
                                Style::default().fg(MUTED)
                            },
                        ),
                        Span::styled(
                            if conv.id == "lobby" {
                                "  Lobby".to_string()
                            } else if online {
                                "  在线".to_string()
                            } else {
                                "  离线".to_string()
                            },
                            Style::default().fg(if online { ONLINE } else { FAINT }),
                        ),
                    ]));
                    if selected {
                        lines.push(Line::from(Span::styled(
                            format!("   {}", truncate(&conv.preview, 46)),
                            Style::default().fg(FAINT),
                        )));
                    }
                }
            }
            Tab::Moments => {
                selected_index = self
                    .moments
                    .iter()
                    .position(|m| Some(&m.moment_id) == self.selected_moment.as_ref())
                    .unwrap_or(0);
                if self.moments.is_empty() {
                    lines.push(Line::from(Span::styled(
                        " 还没有动态，按 n 发布第一条",
                        Style::default().fg(FAINT),
                    )));
                }
                for (index, moment) in self.moments.iter().enumerate() {
                    let selected = index == selected_index;
                    let marker = if selected { "›" } else { " " };
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("{marker} "),
                            Style::default().fg(if selected { ACCENT2 } else { FAINT }),
                        ),
                        Span::styled(
                            format!("🪐 {}", moment.post.author_name),
                            if selected {
                                Style::default()
                                    .fg(Color::White)
                                    .add_modifier(Modifier::BOLD)
                            } else {
                                Style::default().fg(MUTED)
                            },
                        ),
                        Span::styled(
                            format!("  ♥{} 💬{}", moment.like_count, moment.comment_count),
                            Style::default().fg(ACCENT),
                        ),
                    ]));
                    lines.push(Line::from(Span::styled(
                        format!("   {}", truncate(&moment.post.content, 46)),
                        Style::default().fg(FAINT),
                    )));
                }
            }
            Tab::Radar => {
                selected_index = self
                    .peers
                    .iter()
                    .position(|p| Some(&p.peer_id) == self.selected_peer.as_ref())
                    .unwrap_or(0);
                if self.peers.is_empty() {
                    lines.push(Line::from(Span::styled(
                        " 尚未发现其他节点",
                        Style::default().fg(FAINT),
                    )));
                }
                for (index, peer) in self.peers.iter().enumerate() {
                    let selected = index == selected_index;
                    let online = peer.status == "online";
                    let marker = if selected { "›" } else { " " };
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("{marker} "),
                            Style::default().fg(if selected { ACCENT2 } else { FAINT }),
                        ),
                        Span::styled(
                            if online { "● " } else { "○ " },
                            Style::default().fg(if online { ONLINE } else { FAINT }),
                        ),
                        Span::styled(
                            peer.nickname.clone(),
                            if selected {
                                Style::default()
                                    .fg(Color::White)
                                    .add_modifier(Modifier::BOLD)
                            } else {
                                Style::default().fg(MUTED)
                            },
                        ),
                    ]));
                    lines.push(Line::from(Span::styled(
                        format!("   {}:{}", peer.last_ip, peer.last_p2p_port),
                        Style::default().fg(FAINT),
                    )));
                }
            }
        }

        let inner_height = area.height.saturating_sub(2) as usize;
        let offset = selected_index
            .saturating_mul(2)
            .saturating_sub(inner_height.saturating_sub(2)) as u16;
        let paragraph = Paragraph::new(Text::from(lines))
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((offset, 0));
        frame.render_widget(paragraph, area);
    }

    fn draw_detail_panel(&mut self, frame: &mut Frame, area: Rect) {
        let title = match self.tab {
            Tab::Chat => {
                let name = self
                    .convs
                    .iter()
                    .find(|c| c.id == self.selected_conv)
                    .map(|c| c.label.clone())
                    .unwrap_or_else(|| self.selected_conv.clone());
                format!(" {name} ")
            }
            Tab::Moments => " 动态详情 ".to_string(),
            Tab::Radar => " 节点详情 ".to_string(),
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(ACCENT))
            .title(Span::styled(
                title,
                Style::default().fg(ACCENT2).add_modifier(Modifier::BOLD),
            ));

        match self.tab {
            Tab::Chat => self.draw_chat_messages(frame, area, block),
            Tab::Moments => {
                let lines = self.moment_detail_lines();
                frame.render_widget(
                    Paragraph::new(Text::from(lines))
                        .block(block)
                        .wrap(Wrap { trim: false }),
                    area,
                );
            }
            Tab::Radar => {
                let lines = self.radar_detail_lines();
                frame.render_widget(
                    Paragraph::new(Text::from(lines))
                        .block(block)
                        .wrap(Wrap { trim: false }),
                    area,
                );
            }
        }
    }

    fn draw_chat_messages(&mut self, frame: &mut Frame, area: Rect, block: Block<'_>) {
        let mut lines: Vec<Line> = Vec::new();
        if self.messages.is_empty() {
            lines.push(Line::from(Span::styled(
                " 信号已就绪。按 i 输入消息，p 发送图片路径，v 发送语音。",
                Style::default().fg(FAINT),
            )));
        }
        for message in &self.messages {
            let mine = message.sender_id == self.node.peer_id();
            let author = if mine {
                "我".to_string()
            } else {
                message.sender_name.clone()
            };
            let mut spans = vec![
                Span::styled(
                    format!("[{}] ", fmt_age(message.created_at)),
                    Style::default().fg(FAINT),
                ),
                Span::styled(
                    format!("{author}: "),
                    Style::default()
                        .fg(if mine { ACCENT2 } else { ACCENT })
                        .add_modifier(Modifier::BOLD),
                ),
            ];
            match message.msg_type.as_str() {
                "image" => spans.push(Span::styled(
                    format!("[图片] {}", message.content),
                    Style::default().fg(MUTED),
                )),
                "voice" => {
                    let meta = parse_meta(message.media_meta_json.as_deref());
                    let duration = meta
                        .get("duration_ms")
                        .and_then(Value::as_i64)
                        .unwrap_or(0);
                    let wave = sparkline(meta.get("waveform"));
                    spans.push(Span::styled(
                        format!("[语音 {:.1}s] {wave}", duration as f64 / 1000.0),
                        Style::default().fg(Color::Rgb(34, 211, 238)),
                    ));
                }
                _ => spans.push(Span::styled(
                    message.content.clone(),
                    Style::default().fg(Color::White),
                )),
            }
            if mine {
                let (label, color) = match message.delivery_status.as_str() {
                    "delivered" => (" ✓✓", ONLINE),
                    "queued" => (" ◷离线排队", QUEUED),
                    _ => (" ✓", MUTED),
                };
                spans.push(Span::styled(label.to_string(), Style::default().fg(color)));
            }
            lines.push(Line::from(spans));
        }

        if self.input.active
            && matches!(self.input.kind, InputKind::Chat | InputKind::ImagePath)
        {
            let label = match self.input.kind {
                InputKind::ImagePath => "图片路径 > ",
                _ => "消息 > ",
            };
            lines.push(Line::from(vec![
                Span::styled(label, Style::default().fg(QUEUED)),
                Span::styled(self.input.buffer.clone(), Style::default().fg(Color::White)),
                Span::styled("▏", Style::default().fg(ACCENT2)),
            ]));
        }

        let inner_height = area.height.saturating_sub(2) as usize;
        let total = approximate_heights(&lines, area.width.saturating_sub(2));
        if self.auto_scroll {
            self.msg_scroll = total.saturating_sub(inner_height) as u16;
        } else {
            let max = total.saturating_sub(inner_height) as u16;
            if self.msg_scroll > max {
                self.msg_scroll = max;
            }
        }
        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .block(block)
                .wrap(Wrap { trim: false })
                .scroll((self.msg_scroll, 0)),
            area,
        );
    }

    fn moment_detail_lines(&self) -> Vec<Line<'_>> {
        let mut lines: Vec<Line> = Vec::new();
        let moment = match self.selected_moment() {
            Some(moment) => moment,
            None => {
                lines.push(Line::from(Span::styled(
                    " 选择一条动态查看详情；按 n 发布新动态。",
                    Style::default().fg(FAINT),
                )));
                return lines;
            }
        };
        lines.push(Line::from(vec![
            Span::styled("🪐 ", Style::default().fg(ACCENT2)),
            Span::styled(
                moment.post.author_name.clone(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  {}", fmt_age(moment.post.created_at)),
                Style::default().fg(FAINT),
            ),
        ]));
        lines.push(Line::from(Span::styled(
            format!("   {}…", &moment.post.event_id[..moment.post.event_id.len().min(24)]),
            Style::default().fg(FAINT),
        )));
        lines.push(Line::from(""));
        for chunk in wrap_text(&moment.post.content, 60) {
            lines.push(Line::from(Span::styled(
                format!(" {chunk}"),
                Style::default().fg(Color::White),
            )));
        }
        let (images, audios) = media_counts(&moment.media_meta, &moment.blob_ids);
        if images > 0 || audios > 0 {
            lines.push(Line::from(Span::styled(
                format!(" 媒体: 🖼 {images} 张图片 · 🎙 {audios} 段语音 (CAS)"),
                Style::default().fg(ACCENT2),
            )));
        }
        lines.push(Line::from(""));
        let likes = if moment.likes.is_empty() {
            "暂无".to_string()
        } else {
            moment
                .likes
                .iter()
                .map(|event| event.author_name.clone())
                .collect::<Vec<_>>()
                .join(", ")
        };
        lines.push(Line::from(vec![
            Span::styled(format!(" 💖 {} ", moment.like_count), Style::default().fg(DANGER)),
            Span::styled(likes, Style::default().fg(MUTED)),
        ]));
        if !moment.comments.is_empty() {
            for comment in moment.comments.iter().take(12) {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!(" 💬 {}: ", comment.author_name),
                        Style::default().fg(ACCENT2),
                    ),
                    Span::styled(comment.content.clone(), Style::default().fg(MUTED)),
                ]));
            }
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            " l 点赞 · c 评论 · n 发布新动态 · ↑/↓ 切换动态",
            Style::default().fg(FAINT),
        )));
        if self.input.active && matches!(self.input.kind, InputKind::Moment | InputKind::Comment) {
            let label = match self.input.kind {
                InputKind::Comment => "评论 > ",
                _ => "动态正文 > ",
            };
            lines.push(Line::from(vec![
                Span::styled(label, Style::default().fg(QUEUED)),
                Span::styled(self.input.buffer.clone(), Style::default().fg(Color::White)),
                Span::styled("▏", Style::default().fg(ACCENT2)),
            ]));
        }
        lines
    }

    fn radar_detail_lines(&self) -> Vec<Line<'_>> {
        let mut lines = vec![
            Line::from(Span::styled(
                " 📡 节点雷达",
                Style::default().fg(ACCENT2).add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                " 零服务器发现：UDP 48600–48609 回环端口池 + 局域网广播 + 组播",
                Style::default().fg(FAINT),
            )),
            Line::from(""),
        ];
        match self.selected_peer() {
            Some(peer) => {
                lines.push(Line::from(vec![
                    Span::styled(
                        if peer.status == "online" { " ● " } else { " ○ " },
                        Style::default().fg(if peer.status == "online" { ONLINE } else { FAINT }),
                    ),
                    Span::styled(
                        peer.nickname.clone(),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]));
                lines.push(Line::from(Span::styled(
                    format!(" 签名  {}", if peer.bio.is_empty() { "—" } else { &peer.bio }),
                    Style::default().fg(MUTED),
                )));
                lines.push(Line::from(Span::styled(
                    format!(" PeerID  {}", peer.peer_id),
                    Style::default().fg(MUTED),
                )));
                lines.push(Line::from(Span::styled(
                    format!(" 端点    {}:{}", peer.last_ip, peer.last_p2p_port),
                    Style::default().fg(MUTED),
                )));
                lines.push(Line::from(Span::styled(
                    format!(" 动态时钟 {} · 心跳 {}", peer.moments_head_clock, fmt_age(peer.last_seen_at)),
                    Style::default().fg(FAINT),
                )));
            }
            None => lines.push(Line::from(Span::styled(
                " 尚未发现其他节点。在同一局域网启动另一节点即可自动发现。",
                Style::default().fg(FAINT),
            ))),
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            " 监听端口槽位与广播细节可通过 RUST_LOG=nebula_core=debug 查看。",
            Style::default().fg(FAINT),
        )));
        lines
    }

    fn draw_telemetry(&self, frame: &mut Frame, area: Rect) {
        let identity = self.node.identity();
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(ACCENT))
            .title(Span::styled(
                " 节点遥测 / CAS ",
                Style::default().fg(ACCENT2).add_modifier(Modifier::BOLD),
            ));
        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(Span::styled(
            " 身份",
            Style::default().fg(ACCENT2).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            format!("  昵称    {}", identity.nickname),
            Style::default().fg(MUTED),
        )));
        lines.push(Line::from(Span::styled(
            format!("  PeerID  {}", identity.peer_id),
            Style::default().fg(MUTED),
        )));
        lines.push(Line::from(Span::styled(
            format!("  Lamport {}", identity.lamport_clock),
            Style::default().fg(FAINT),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            " 网络",
            Style::default().fg(ACCENT2).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            format!("  在线     {} / 已知 {}", self.online_peers(), self.peers.len()),
            Style::default().fg(ONLINE),
        )));
        lines.push(Line::from(Span::styled(
            format!("  P2P      :{}", self.node.p2p_port),
            Style::default().fg(MUTED),
        )));
        lines.push(Line::from(Span::styled(
            format!("  UI       http://127.0.0.1:{}", self.node.ui_port),
            Style::default().fg(MUTED),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            " 内容寻址存储",
            Style::default().fg(ACCENT2).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            format!("  媒体     {} 个", self.blob_count),
            Style::default().fg(MUTED),
        )));
        lines.push(Line::from(Span::styled(
            format!("  体积     {}", fmt_bytes(self.blob_bytes)),
            Style::default().fg(MUTED),
        )));
        lines.push(Line::from(Span::styled(
            format!(
                "  待拉取   {}",
                self.pending_blobs
            ),
            Style::default().fg(if self.pending_blobs > 0 { QUEUED } else { MUTED }),
        )));
        lines.push(Line::from(Span::styled(
            format!("  数据目录 {}", shorten_path(&self.node.cfg.data_dir.display().to_string(), 26)),
            Style::default().fg(FAINT),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            " 本地 API",
            Style::default().fg(ACCENT2).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            "  /api/p2p/chat  /api/p2p/sync",
            Style::default().fg(FAINT),
        )));
        lines.push(Line::from(Span::styled(
            "  /api/p2p/blob/:sha256",
            Style::default().fg(FAINT),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            " 版本向量",
            Style::default().fg(ACCENT2).add_modifier(Modifier::BOLD),
        )));
        if self.moments.is_empty() {
            lines.push(Line::from(Span::styled(
                "  暂无事件",
                Style::default().fg(FAINT),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                format!("  动态 {} 条 · 已聚合", self.moments.len()),
                Style::default().fg(MUTED),
            )));
        }
        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .block(block)
                .wrap(Wrap { trim: false }),
            area,
        );
    }

    fn draw_footer(&self, frame: &mut Frame, area: Rect, wide: bool) {
        let mut spans = vec![
            Span::styled(" ⏺ ", Style::default().fg(ACCENT2)),
            Span::styled(self.status.clone(), Style::default().fg(MUTED)),
        ];
        if wide {
            spans.push(Span::styled(
                "   │   ↑/↓ 选择  i 输入  p 图片  v 语音  n 动态  l 点赞  c 评论  q 退出",
                Style::default().fg(FAINT),
            ));
        } else {
            spans.push(Span::styled(
                "  │  F1/F2/F3 面板",
                Style::default().fg(FAINT),
            ));
        }
        if self.input.active {
            spans.push(Span::styled(
                "   [输入中 — Enter 提交 / Esc 取消]",
                Style::default().fg(QUEUED),
            ));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }
}

// ----------------------------------------------------------------------
// Helpers
// ----------------------------------------------------------------------
fn short_id(id: &str) -> String {
    let cleaned: String = id.chars().filter(|c| *c != ':').take(8).collect();
    format!("{cleaned}…")
}

fn message_preview(message: &MessageRecord) -> String {
    match message.msg_type.as_str() {
        "image" => format!("[图片] {}", message.content),
        "voice" => "[语音]".to_string(),
        _ => message.content.clone(),
    }
}

fn delivery_label(status: &str) -> &'static str {
    match status {
        "delivered" => "已送达",
        "queued" => "离线排队中",
        _ => "已发送",
    }
}

fn fmt_age(ts: i64) -> String {
    let diff = (nebula_core::now_ms() - ts).max(0) / 1000;
    if diff < 60 {
        format!("{diff}s")
    } else if diff < 3600 {
        format!("{}m", diff / 60)
    } else if diff < 86_400 {
        format!("{}h", diff / 3600)
    } else {
        format!("{}d", diff / 86_400)
    }
}

fn fmt_bytes(value: i64) -> String {
    let value = value.max(0) as f64;
    if value < 1024.0 {
        format!("{} B", value as i64)
    } else if value < 1024.0 * 1024.0 {
        format!("{:.1} KB", value / 1024.0)
    } else if value < 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} MB", value / 1024.0 / 1024.0)
    } else {
        format!("{:.2} GB", value / 1024.0 / 1024.0 / 1024.0)
    }
}

fn truncate(text: &str, max_chars: usize) -> String {
    let cleaned = text.replace('\n', " ");
    let mut chars = cleaned.chars();
    let head: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

fn shorten_path(path: &str, max_chars: usize) -> String {
    if path.chars().count() <= max_chars {
        path.to_string()
    } else {
        let tail: String = path
            .chars()
            .rev()
            .take(max_chars.saturating_sub(1))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        format!("…{tail}")
    }
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let mut result = Vec::new();
    for paragraph in text.lines() {
        if paragraph.is_empty() {
            result.push(String::new());
            continue;
        }
        let mut line = String::new();
        for ch in paragraph.chars() {
            line.push(ch);
            if line.chars().count() >= width {
                result.push(line.clone());
                line.clear();
            }
        }
        if !line.is_empty() {
            result.push(line);
        }
    }
    if result.is_empty() {
        result.push(String::new());
    }
    result
}

fn parse_meta(raw: Option<&str>) -> Value {
    raw.and_then(|text| serde_json::from_str(text).ok())
        .unwrap_or(Value::Null)
}

fn sparkline(waveform: Option<&Value>) -> String {
    let values: Vec<i64> = waveform
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default();
    if values.is_empty() {
        return "▁▂▃▄▅▆▇█".chars().take(8).collect();
    }
    values
        .iter()
        .take(16)
        .map(|value| {
            let index = ((*value).clamp(0, 100) as usize * (SPARK.len() - 1)) / 100;
            SPARK[index]
        })
        .collect()
}

fn media_counts(meta: &Value, blob_ids: &[String]) -> (usize, usize) {
    let mimes = meta.get("mimes");
    let mut images = 0;
    let mut audios = 0;
    for id in blob_ids {
        let mime = mimes
            .and_then(|m| m.get(id))
            .and_then(Value::as_str)
            .unwrap_or("image/*");
        if mime.starts_with("audio/") {
            audios += 1;
        } else {
            images += 1;
        }
    }
    (images, audios)
}

fn approximate_heights(lines: &[Line<'_>], width: u16) -> usize {
    let width = width.max(1) as usize;
    lines
        .iter()
        .map(|line| {
            let chars: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
            chars.div_ceil(width).max(1)
        })
        .sum()
}

/// Synthesize a 1.6 s WAV voice note with a 16-bar envelope waveform so the
/// TUI can exercise the full media pipeline without native audio bindings.
fn synth_voice() -> (Vec<u8>, Vec<i64>, i64) {
    let sample_rate: u32 = 8_000;
    let samples: u32 = sample_rate * 16 / 10;
    let mut wav = Vec::with_capacity(44 + samples as usize * 2);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + samples * 2).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(samples * 2).to_le_bytes());
    for i in 0..samples {
        let t = i as f32 / sample_rate as f32;
        let envelope = (std::f32::consts::PI * i as f32 / samples as f32).sin();
        let sample = (t * 440.0 * std::f32::consts::TAU).sin() * 0.35 * envelope;
        wav.extend_from_slice(&((sample * i16::MAX as f32) as i16).to_le_bytes());
    }
    let waveform: Vec<i64> = (0..16)
        .map(|i| {
            let envelope = (std::f64::consts::PI * i as f64 / 16.0).sin();
            (10.0 + envelope * 90.0).round() as i64
        })
        .collect();
    (wav, waveform, samples as i64 * 1000 / sample_rate as i64)
}

// ----------------------------------------------------------------------
// Entry point
// ----------------------------------------------------------------------
pub async fn run_tui(node: Arc<NebulaNode>) -> Result<()> {
    let mut app = TuiApp::new(node.clone());
    app.refresh();

    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = match Terminal::new(backend) {
        Ok(terminal) => terminal,
        Err(err) => {
            let _ = disable_raw_mode();
            return Err(err.into());
        }
    };

    let result = event_loop(&mut terminal, &mut app).await;

    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();
    node.shutdown().await;
    result
}

async fn event_loop<B>(terminal: &mut Terminal<B>, app: &mut TuiApp) -> Result<()>
where
    B: ratatui::backend::Backend,
{
    let mut last_refresh = Instant::now();
    loop {
        terminal.draw(|frame| app.draw(frame))?;
        if app.should_quit {
            break;
        }
        if event::poll(Duration::from_millis(60))? {
            if let Event::Key(key) = event::read()? {
                app.handle_key(key).await?;
            }
        }
        if last_refresh.elapsed() >= Duration::from_millis(400) {
            app.refresh();
            last_refresh = Instant::now();
        }
    }
    Ok(())
}

/// Convenience helper for the desktop host: boot a node from CLI options and
/// run the TUI with it.
pub async fn boot_and_run(config: nebula_core::NebulaConfig) -> Result<()> {
    let node = NebulaNode::start(config).await?;
    run_tui(node).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use nebula_core::NebulaConfig;
    use ratatui::backend::TestBackend;
    use tempfile::TempDir;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_text(text: &str) -> Vec<KeyEvent> {
        text.chars().map(|ch| key(KeyCode::Char(ch))).collect()
    }

    fn render(terminal: &mut Terminal<TestBackend>, app: &mut TuiApp) -> String {
        terminal.draw(|frame| app.draw(frame)).expect("draw");
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn renders_all_form_factors_and_drives_actions() {
        let dir: TempDir = tempfile::tempdir().unwrap();
        let mut cfg = NebulaConfig::new(dir.path());
        cfg.nickname = Some("Tui-Test".to_string());
        cfg.p2p_port = 0;
        cfg.ui_port = 0;
        cfg.heartbeat = Duration::from_secs(1);
        cfg.sync_interval = Duration::from_millis(400);
        let node = NebulaNode::start(cfg).await.unwrap();
        let mut app = TuiApp::new(node.clone());
        app.refresh();

        for (width, height) in [(160u16, 48u16), (120, 40), (99, 30), (80, 24), (60, 20)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let content = render(&mut terminal, &mut app);
            // Wide CJK glyphs occupy two cells; the continuation cell is a
            // space, so compare on whitespace-stripped text.
            let compact: String = content.chars().filter(|ch| !ch.is_whitespace()).collect();
            assert!(content.contains("NEBULA"), "brand missing at {width}x{height}");
            assert!(
                compact.contains("星云大厅") || compact.contains("星讯"),
                "chat UI missing at {width}x{height}"
            );
        }

        // Switch tabs at every width.
        for code in [KeyCode::F(2), KeyCode::Char('3'), KeyCode::Char('1')] {
            app.handle_key(key(code)).await.unwrap();
        }
        assert_eq!(app.tab, Tab::Chat);

        // Compose a chat message.
        app.handle_key(key(KeyCode::Char('i'))).await.unwrap();
        for event in type_text("Hello TUI") {
            app.handle_key(event).await.unwrap();
        }
        app.handle_key(key(KeyCode::Enter)).await.unwrap();
        assert!(
            app.messages.iter().any(|m| m.content == "Hello TUI"),
            "chat message never stored"
        );

        // Send a synthesized voice note through the CAS pipeline.
        app.handle_key(key(KeyCode::Char('v'))).await.unwrap();
        app.refresh();
        assert!(
            app.messages.iter().any(|m| m.msg_type == "voice"),
            "voice message never stored"
        );

        // Send a local image path.
        let image_path = dir.path().join("nebula-test.png");
        std::fs::write(&image_path, b"tui-image-bytes").unwrap();
        app.handle_key(key(KeyCode::Char('p'))).await.unwrap();
        for event in type_text(&image_path.display().to_string()) {
            app.handle_key(event).await.unwrap();
        }
        app.handle_key(key(KeyCode::Enter)).await.unwrap();
        assert!(
            app.messages.iter().any(|m| m.msg_type == "image"),
            "image message never stored"
        );

        // Publish + like a moment.
        app.handle_key(key(KeyCode::Char('n'))).await.unwrap();
        assert_eq!(app.tab, Tab::Moments);
        for event in type_text("TUI moment") {
            app.handle_key(event).await.unwrap();
        }
        app.handle_key(key(KeyCode::Enter)).await.unwrap();
        assert!(!app.moments.is_empty(), "moment never stored");
        let moment_id = app.selected_moment.clone().unwrap();
        app.handle_key(key(KeyCode::Char('l'))).await.unwrap();
        let liked = app
            .moments
            .iter()
            .find(|m| m.moment_id == moment_id)
            .map(|m| m.liked_by_me)
            .unwrap_or(false);
        assert!(liked, "moment like was not applied");

        // Comment on it.
        app.handle_key(key(KeyCode::Char('c'))).await.unwrap();
        for event in type_text("Nice nebula") {
            app.handle_key(event).await.unwrap();
        }
        app.handle_key(key(KeyCode::Enter)).await.unwrap();
        let commented = app
            .moments
            .iter()
            .find(|m| m.moment_id == moment_id)
            .map(|m| m.comments.iter().any(|e| e.content == "Nice nebula"))
            .unwrap_or(false);
        assert!(commented, "comment was not stored");

        // Narrow render while moments are populated.
        let mut terminal = Terminal::new(TestBackend::new(88, 26)).unwrap();
        let content = render(&mut terminal, &mut app);
        let compact: String = content.chars().filter(|ch| !ch.is_whitespace()).collect();
        assert!(content.contains("NEBULA"));
        assert!(compact.contains("星云圈") || compact.contains("动态"));

        app.handle_key(key(KeyCode::Char('q'))).await.unwrap();
        assert!(app.should_quit);
        node.shutdown().await;
    }
}
