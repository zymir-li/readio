//! Chrome: the header bar, the status line, and the help overlay.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block as UiBlock, BorderType, Borders, Clear, Paragraph, Widget};

use crate::i18n::{t, tf};
use crate::metrics;
use crate::theme::{self, theme};
use crate::wrap::{display_width, truncate};

/// Everything the chrome needs to know about the session.
pub struct Chrome<'a> {
    pub model_label: &'a str,
    pub directory: &'a str,
    pub branch: &'a str,
    pub book: &'a str,
    pub author: Option<&'a str>,
    pub chapter_title: &'a str,
    pub chapter_index: usize,
    pub chapter_total: usize,
    pub progress: f32,
    pub cps: f32,
    pub session_chars: usize,
    /// Wall-clock time since readio started, shown like an agent's turn timer.
    pub elapsed: std::time::Duration,
    /// Engine name while read-aloud is on.
    pub speaking: Option<&'a str>,
    /// Read-aloud is on but held back because the output device is not on the
    /// whitelist. Shown wherever the engine name would be, because a silent
    /// mute is indistinguishable from a broken engine.
    pub audio_muted: bool,
    /// Manual, auto or read-aloud, shown as the chip `shift+tab` cycles.
    pub mode: crate::mode::Mode,
    /// Reading pace, shown where a coding agent shows reasoning effort: beside the
    /// model name.
    pub effort: crate::effort::Effort,
    pub busy: bool,
    /// Held where it is, waiting for the reader to come back.
    pub paused: bool,
    pub tick: u64,
    pub notice: Option<&'a str>,
    pub scrolled: bool,
    pub scroll_percent: f32,
    /// False before a book is opened, when readio is showing the library.
    pub has_book: bool,
    pub library_count: usize,
}

/// Lay a left-aligned and a right-aligned run of spans on one row.
fn split_row(area: Rect, buf: &mut Buffer, left: Vec<Span<'static>>, right: Vec<Span<'static>>) {
    let right_width: usize = right.iter().map(|s| display_width(&s.content)).sum();
    let left_width = (area.width as usize).saturating_sub(right_width + 1);

    let mut trimmed = Vec::new();
    let mut used = 0usize;
    for span in left {
        let w = display_width(&span.content);
        if used + w > left_width {
            let room = left_width.saturating_sub(used);
            if room > 1 {
                trimmed.push(Span::styled(truncate(&span.content, room), span.style));
            }
            break;
        }
        used += w;
        trimmed.push(span);
    }
    Paragraph::new(Line::from(trimmed)).render(area, buf);

    if right_width <= area.width as usize {
        let x = area.right() - right_width as u16;
        let rect = Rect::new(x, area.y, right_width as u16, 1);
        Paragraph::new(Line::from(right)).render(rect, buf);
    }
}

pub fn render_header(area: Rect, buf: &mut Buffer, c: &Chrome<'_>) {
    let th = theme();
    let left = vec![Span::styled(
        " ● session",
        Style::default().fg(th.text_secondary),
    )];
    let right = if c.has_book {
        vec![Span::styled(
            format!("ctx {:.1}% ", metrics::context_percent(c.progress)),
            Style::default().fg(th.accent_model),
        )]
    } else {
        vec![Span::styled(
            tf("chrome.library_count", &[&c.library_count]),
            Style::default().fg(th.text_secondary),
        )]
    };
    split_row(area, buf, left, right);
}

/// Rows the activity strip needs: one while the turn is interrupted, none otherwise.
///
/// An interruption belongs at the end of the transcript — directly under the last
/// words, above the input box — because that is where everything the agent is
/// doing, or has stopped doing, appears. It used to sit in the footer beside the
/// key hints, which read as a label on the prompt rather than as a state the turn
/// was in.
pub fn activity_height(c: &Chrome<'_>) -> u16 {
    u16::from(c.paused)
}

/// Draw that strip: what happened, and the key that undoes it.
///
/// No spinner. A spinner means work is going on, and the whole point of this
/// state is that none is: the turn is stopped and waiting for the reader. The
/// way back is printed along the status line with every other key hint.
pub fn render_activity(area: Rect, buf: &mut Buffer, c: &Chrome<'_>) {
    if area.height == 0 || !c.paused {
        return;
    }
    let th = theme();
    let line = Line::from(vec![
        Span::styled(
            format!(" {} ", theme::QUOTE_BAR),
            Style::default().fg(th.accent_warning),
        ),
        Span::styled(
            t("chrome.paused").to_string(),
            Style::default().fg(th.accent_thinking),
        ),
    ]);
    Paragraph::new(vec![line]).render(area, buf);
}

/// Read-aloud indicator: the engine when it is sounding, a struck-through note
/// and the reason when the output device is not allowed.
fn speech_span(c: &Chrome<'_>) -> Vec<Span<'static>> {
    let th = theme();
    if c.audio_muted {
        return vec![Span::styled(
            format!("  {} {}", theme::NOTE_MUTED, t("dev.muted_status")),
            Style::default().fg(th.accent_warning),
        )];
    }
    match c.speaking {
        Some(engine) => vec![Span::styled(
            format!("  {} {engine}", theme::PLAY),
            Style::default().fg(th.accent_plan),
        )],
        None => Vec::new(),
    }
}

pub fn render_status(area: Rect, buf: &mut Buffer, c: &Chrome<'_>) {
    if area.height == 0 {
        return;
    }
    let th = theme();
    let muted = Style::default().fg(th.text_secondary);
    let label = if c.model_label.trim().is_empty() {
        "local"
    } else {
        c.model_label.trim()
    };
    let first = vec![
        Span::styled(
            format!(" [model: {} · {}]", truncate(label, 24), c.effort.code()),
            Style::default().fg(th.accent_model),
        ),
        Span::styled(format!("  {}", truncate(c.directory, 22)), muted),
        Span::styled(
            format!("  git:({})", truncate(c.branch, 28)),
            Style::default().fg(th.accent_thinking),
        ),
    ];
    split_row(
        Rect::new(area.x, area.y, area.width, 1),
        buf,
        first,
        vec![Span::styled(
            format!(" {} ", metrics::format_duration(c.elapsed)),
            muted,
        )],
    );
    if area.height < 2 {
        return;
    }
    let mut second = if let Some(notice) = c.notice {
        vec![Span::styled(format!(" ● {notice}"), muted)]
    } else if c.paused {
        vec![Span::styled(t("chrome.paused_keys").to_string(), muted)]
    } else if c.scrolled {
        vec![Span::styled(
            tf(
                "chrome.scrolled",
                &[
                    &theme::ELLIPSIS,
                    &format!("{:.0}", c.scroll_percent * 100.0),
                ],
            ),
            muted,
        )]
    } else {
        let filled = (c.progress.clamp(0.0, 1.0) * 8.0).round() as usize;
        vec![
            Span::styled(
                format!(
                    " Usage ~{} tok  ·  ",
                    metrics::format_tokens(metrics::tokens_from_chars(c.session_chars))
                ),
                muted,
            ),
            Span::styled(
                format!(
                    "ctx [{}{}] {:.1}%",
                    "━".repeat(filled),
                    "─".repeat(8 - filled),
                    metrics::context_percent(c.progress)
                ),
                Style::default().fg(th.accent_model),
            ),
        ]
    };
    second.extend(speech_span(c));
    split_row(
        Rect::new(area.x, area.y + 1, area.width, 1),
        buf,
        second,
        vec![Span::styled(
            if c.mode == crate::mode::Mode::Speak {
                " ← / → · space pause "
            } else if c.paused {
                " Enter / space · resume "
            } else {
                " ↵ continue · /help "
            },
            Style::default().fg(th.text_faint),
        )],
    );
}

/// Help rows: label, Chinese, English. Labels are keys and commands, which are
/// the same in every language.
#[rustfmt::skip]
const HELP: &[(&str, &str, &str)] = &[
    ("⏎", "载入下一段；命令以 / 开头", "load the next passage; commands start with /"),
    ("⏎ (输出中 / mid-turn)", "加速当前这一轮，直接读到底", "rush the current turn to its end"),
    ("esc", "中断这一轮，位置留在原处；回车接着读", "interrupt the turn, keeping your place; ⏎ carries on"),
    ("空格 space", "原地暂停 / 从同一音频位置继续（输入框是空的时候）",
        "pause in place / resume at the same audio position (when the line is empty)"),
    ("[ / ]", "朗读减速 / 加速", "slower / faster read-aloud"),
    ("shift+tab", "循环手动 / 自动 / 朗读模式",
        "cycle manual / auto / read-aloud"),
    ("↑ ↓ / 滚轮 wheel", "滚动；手动模式下滚到底会载入下一段",
        "scroll; in manual mode, at the bottom it loads more"),
    ("pgup pgdn / home end", "翻页；到顶 / 到底", "by page; to the top / to the tail"),
    ("^p ^n", "翻输入历史", "input history"),
    ("^t", "展开/折叠思考过程", "fold or unfold reasoning"),
    ("^o", "展开/折叠工具调用", "fold or unfold tool calls"),
    ("^s", "进入朗读 / 返回之前模式", "enter read-aloud / return to the previous mode"),
    ("^r", "推理强度下一档，也就是读快一点/慢一点", "next reasoning effort: read faster or slower"),
    ("^l", "清屏", "clear the screen"),
    ("^c ^d", "退出；正在输出时 ^c 先把这一轮丢掉（esc 只是中断，还能接着读）",
        "quit; while running, ^c discards the turn (esc only interrupts — that one resumes)"),
    ("", "", ""),
    ("/lib", "列出书库", "list the library"),
    ("/open <n>", "打开第几本；直接输序号也行", "open entry n; a bare number works too"),
    ("/import <path>", "导入；--copy 复制、--link 记路径、--move 搬进来",
        "import; --copy, --link or --move"),
    ("/forget <n>", "从书库移除", "drop an entry from the library"),
    ("/sample", "打开内置示例", "open the built-in sample"),
    ("", "", ""),
    ("/toc", "列出章节", "table of contents"),
    ("/goto <n>", "跳到第 n 章", "jump to chapter n"),
    ("/next /prev", "下一章 / 上一章", "next / previous chapter"),
    ("/find <term>", "全书检索；输序号跳到那一处", "search the book; type a number to jump"),
    ("^g ^b", "下一处 / 上一处命中", "next / previous hit"),
    ("/mode [manual|auto|aloud]", "手动 / 自动 / 朗读；shift+tab 循环",
        "manual / auto / read-aloud; shift+tab cycles"),
    ("/plan", "章节清单", "chapters as a plan"),
    ("/context", "阅读上下文", "context window readout"),
    ("/progress", "进度摘要", "one-line progress"),
    ("", "", ""),
    ("/mark [note]", "标记现在这一处", "keep this place"),
    ("/marks [n]", "列出标记；带序号则跳过去", "list marks; with a number, go to one"),
    ("/unmark <n>", "删掉一个标记", "drop a mark"),
    ("", "", ""),
    ("/effort [level]", "推理强度：minimal…max，越高读得越慢",
        "reasoning effort: minimal…max, higher reads slower"),
    ("/voice [auto|zh|en|音色|引擎]", "谁来念：引擎、音色、语种；没装的当场装",
        "who reads: engine, voice, language; installs one that is missing"),
    ("/rate <0.5-3>", "改当前强度这一档的倍数", "retune the current effort level"),
    ("/device", "音频输出白名单：只在指定设备上出声",
        "audio output whitelist: only speak on devices you name"),
    ("/speed <n>", "吐字速度（朗读时跟随音频）",
        "reveal speed (follows the audio while reading aloud)"),
    ("/lang zh|en", "界面语言", "interface language"),
    ("/clear", "清屏，进度不受影响", "clear the screen; your place is kept"),
    ("/help  /quit", "这个面板 / 退出", "this panel / quit"),
];

/// The help rows in the current language.
fn help_rows() -> Vec<(&'static str, &'static str)> {
    let zh = matches!(crate::i18n::current(), crate::i18n::Lang::Zh);
    HELP.iter()
        .map(|(label, chinese, english)| (*label, if zh { *chinese } else { *english }))
        .collect()
}

pub fn render_help(area: Rect, buf: &mut Buffer) {
    let th = theme();
    let rows = help_rows();
    let width = 72u16.min(area.width.saturating_sub(4)).max(24);
    let height = (rows.len() as u16 + 4).min(area.height.saturating_sub(2));
    let rect = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    );

    Clear.render(rect, buf);
    let frame = UiBlock::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(th.border_focus))
        .title(Span::styled(
            t("chrome.help_title").to_string(),
            Style::default().fg(th.accent_agent).bold(),
        ))
        .style(Style::default().bg(th.bg_soft));
    let inner = frame.inner(rect);
    frame.render(rect, buf);

    let key_col = rows
        .iter()
        .map(|(label, _)| display_width(label))
        .max()
        .unwrap_or(12)
        + 2;
    let lines: Vec<Line<'static>> = rows
        .iter()
        .map(|(key, desc)| {
            if key.is_empty() {
                return Line::from("");
            }
            let pad = key_col.saturating_sub(display_width(key));
            Line::from(vec![
                Span::styled(
                    format!(" {key}{}", " ".repeat(pad)),
                    Style::default().fg(th.accent_model),
                ),
                Span::styled(desc.to_string(), Style::default().fg(th.text_secondary)),
            ])
        })
        .collect();
    Paragraph::new(lines).render(inner, buf);

    if inner.height > rows.len() as u16 {
        let hint = Rect::new(inner.x, inner.bottom() - 1, inner.width, 1);
        Paragraph::new(Line::from(Span::styled(
            t("chrome.help_close").to_string(),
            Style::default().fg(th.text_faint),
        )))
        .render(hint, buf);
    }
}
