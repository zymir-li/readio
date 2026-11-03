//! The scrollback surface: an ordered list of blocks, a per-entry line cache,
//! and a viewport that follows the tail until the reader scrolls away.

use std::collections::HashMap;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};

use super::block::{Block, Ctx, Highlight, ImagePlan, Mode};
use super::image::{TermImage, placeholder};

/// Rendered lines for one entry, keyed by the inputs that produced them.
struct Cache {
    width: u16,
    generation: u64,
    /// `Some(tick)` when the entry animates, so a new frame invalidates it.
    tick: Option<u64>,
    lines: Vec<Line<'static>>,
}

pub struct Entry {
    pub id: u64,
    pub block: Block,
    pub running: bool,
    pub mode: Mode,
    generation: u64,
    cache: Option<Cache>,
}

impl Entry {
    fn touch(&mut self) {
        self.generation += 1;
    }

    fn lines(&mut self, width: u16, tick: u64) -> &[Line<'static>] {
        let animated = self.block.animated(self.running);
        let fresh = self.cache.as_ref().is_some_and(|c| {
            c.width == width
                && c.generation == self.generation
                && (!animated || c.tick == Some(tick))
        });
        if !fresh {
            let ctx = Ctx {
                width,
                tick,
                running: self.running,
                mode: self.mode,
            };
            self.cache = Some(Cache {
                width,
                generation: self.generation,
                tick: animated.then_some(tick),
                lines: self.block.render(&ctx),
            });
        }
        &self.cache.as_ref().expect("cache filled above").lines
    }
}

/// Upper bound on retained entries; a long session should not grow forever.
const MAX_ENTRIES: usize = 4_000;

pub struct Scrollback {
    entries: Vec<Entry>,
    next_id: u64,
    /// Stick to the bottom as new content arrives.
    pub follow: bool,
    /// First visible line index when not following.
    offset: usize,
    /// Line count of the last laid-out frame.
    total_lines: usize,
    /// Viewport height of the last laid-out frame.
    viewport: usize,
    width: u16,
    anchor_next: bool,
    pending_anchor: Option<u64>,
    image_buttons: Vec<(Rect, std::path::PathBuf)>,
    /// Global fold preference for tool blocks.
    pub expand_tools: bool,
    /// Decoded illustrations, keyed by entry and the size they were fitted to.
    /// Decoding is the expensive part, so it happens once per size, not once
    /// per frame.
    images: HashMap<(u64, u16, u16), Option<TermImage>>,
}

impl Default for Scrollback {
    fn default() -> Self {
        Self::new()
    }
}

impl Scrollback {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            next_id: 1,
            follow: true,
            offset: 0,
            total_lines: 0,
            viewport: 0,
            width: 80,
            anchor_next: false,
            pending_anchor: None,
            image_buttons: Vec::new(),
            expand_tools: false,
            images: HashMap::new(),
        }
    }

    /// Use two thirds of the actual text viewport for each manual reading group.
    pub fn reading_window(&self) -> (u16, usize) {
        (
            self.width.max(8),
            (self.viewport.saturating_sub(2)).clamp(3, 7),
        )
    }

    pub fn image_at(&self, x: u16, y: u16) -> Option<std::path::PathBuf> {
        self.image_buttons
            .iter()
            .find(|(rect, _)| x >= rect.x && x < rect.right() && y >= rect.y && y < rect.bottom())
            .map(|(_, path)| path.clone())
    }

    pub fn begin_reading(&mut self) {
        self.anchor_next = true;
        self.pending_anchor = None;
    }

    pub fn push(&mut self, block: Block) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        if self.anchor_next && matches!(&block, Block::Passage { .. } | Block::Image { .. }) {
            self.pending_anchor = Some(id);
            self.anchor_next = false;
        }
        let mode = match block {
            Block::Tool(_) if !self.expand_tools => Mode::Collapsed,
            Block::Tool(_) => Mode::Expanded,
            _ => Mode::Truncated,
        };
        self.entries.push(Entry {
            id,
            block,
            running: false,
            mode,
            generation: 0,
            cache: None,
        });
        if self.entries.len() > MAX_ENTRIES {
            let drop = self.entries.len() - MAX_ENTRIES;
            self.entries.drain(..drop);
        }
        self.follow = true;
        id
    }

    /// Push a block that starts in the running state (spinner, live cursor).
    pub fn push_running(&mut self, block: Block) -> u64 {
        let id = self.push(block);
        if let Some(entry) = self.entry_mut(id) {
            entry.running = true;
            entry.touch();
        }
        id
    }

    fn entry_mut(&mut self, id: u64) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|e| e.id == id)
    }

    /// Point the read-aloud highlight at one block's sentence and word.
    pub fn set_highlight(&mut self, id: u64, state: Option<Highlight>) -> bool {
        let Some(entry) = self.entry_mut(id) else {
            return false;
        };
        if entry.block.set_highlight(state) {
            entry.touch();
            return true;
        }
        false
    }

    /// Clear every highlight, for when speech stops.
    pub fn clear_highlight(&mut self) {
        for entry in self.entries.iter_mut() {
            if entry.block.set_highlight(None) {
                entry.touch();
            }
        }
    }

    pub fn push_chunk(&mut self, id: u64, chunk: &str) -> bool {
        let Some(entry) = self.entry_mut(id) else {
            return false;
        };
        let ok = entry.block.push_chunk(chunk);
        if ok {
            entry.touch();
            self.follow_tail();
        }
        ok
    }

    /// Number of characters streamed into a block so far.
    fn char_len_of(&self, id: u64) -> usize {
        self.entries
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.block.char_len())
            .unwrap_or(0)
    }

    /// Add a line of live output to a running tool call.
    pub fn push_output(&mut self, id: u64, line: String) -> bool {
        let Some(entry) = self.entry_mut(id) else {
            return false;
        };
        let ok = entry.block.push_output(line);
        if ok {
            entry.touch();
            self.follow_tail();
        }
        ok
    }

    /// Mark a running tool call as failed, with the reason on its header.
    pub fn fail_tool(&mut self, id: u64, message: String) {
        if let Some(entry) = self.entry_mut(id) {
            entry.running = false;
            if entry.block.fail_tool(message) {
                entry.touch();
            }
        }
    }

    pub fn finish(&mut self, id: u64, elapsed_ms: Option<u64>) {
        if let Some(entry) = self.entry_mut(id) {
            entry.running = false;
            if let Some(ms) = elapsed_ms {
                entry.block.set_elapsed(ms);
                entry.block.finish_tool(ms);
            }
            // A finished thinking block folds itself away.
            if matches!(entry.block, Block::Thinking { .. }) {
                entry.mode = Mode::Collapsed;
            }
            entry.touch();
        }
    }

    /// Drop a block that never received content (an empty streaming placeholder).
    pub fn remove_if_empty(&mut self, id: u64) -> bool {
        if self.char_len_of(id) > 0 {
            return false;
        }
        let Some(pos) = self.entries.iter().position(|e| e.id == id) else {
            return false;
        };
        self.entries.remove(pos);
        true
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.image_buttons.clear();
        self.anchor_next = false;
        self.pending_anchor = None;
        self.images.clear();
        self.offset = 0;
        self.total_lines = 0;
        self.follow = true;
    }

    /// Drop every cached line, so a language or theme change is picked up on
    /// the next frame instead of leaving stale text on screen.
    pub fn invalidate(&mut self) {
        for entry in self.entries.iter_mut() {
            entry.touch();
        }
    }

    /// Toggle fold state of tool blocks globally (Ctrl+O).
    pub fn toggle_tool_fold(&mut self) {
        self.expand_tools = !self.expand_tools;
        let mode = if self.expand_tools {
            Mode::Expanded
        } else {
            Mode::Collapsed
        };
        for entry in self.entries.iter_mut() {
            if matches!(entry.block, Block::Tool(_)) {
                entry.mode = mode;
                entry.touch();
            }
        }
    }

    /// Toggle visibility of finished reasoning blocks (Ctrl+T).
    pub fn toggle_thinking_fold(&mut self) {
        let any_collapsed = self
            .entries
            .iter()
            .any(|e| matches!(e.block, Block::Thinking { .. }) && e.mode == Mode::Collapsed);
        for entry in self.entries.iter_mut() {
            if matches!(entry.block, Block::Thinking { .. }) {
                entry.mode = if any_collapsed {
                    Mode::Expanded
                } else {
                    Mode::Collapsed
                };
                entry.touch();
            }
        }
    }

    // ── scrolling ────────────────────────────────────────────────────────────

    fn follow_tail(&mut self) {
        if self.follow {
            self.offset = self.total_lines.saturating_sub(self.viewport);
        }
    }

    pub fn scroll_lines(&mut self, delta: i32) {
        self.pending_anchor = None;
        self.anchor_next = false;
        let max = self.max_offset();
        let next = (self.offset as i64 + delta as i64).clamp(0, max as i64) as usize;
        self.offset = next;
        self.follow = next >= max;
    }

    pub fn page(&mut self, direction: i32) {
        let step = (self.viewport as i32 - 2).max(1) * direction;
        self.scroll_lines(step);
    }

    pub fn to_bottom(&mut self) {
        self.pending_anchor = None;
        self.anchor_next = false;
        self.offset = self.max_offset();
        self.follow = true;
    }

    pub fn to_top(&mut self) {
        self.pending_anchor = None;
        self.anchor_next = false;
        self.offset = 0;
        self.follow = false;
    }

    fn max_offset(&self) -> usize {
        self.total_lines.saturating_sub(self.viewport)
    }

    /// True when content extends above the viewport (drives the scroll hint).
    pub fn scrolled_away(&self) -> bool {
        !self.follow && self.total_lines > self.viewport
    }

    pub fn scroll_percent(&self) -> f32 {
        let max = self.max_offset();
        if max == 0 {
            return 1.0;
        }
        self.offset as f32 / max as f32
    }

    // ── rendering ────────────────────────────────────────────────────────────

    pub fn render(&mut self, area: Rect, buf: &mut Buffer, tick: u64) {
        self.image_buttons.clear();
        if area.width < 8 || area.height == 0 {
            return;
        }
        self.viewport = area.height as usize;
        self.width = area.width;

        // Pass one: lay out every entry as lines, noting where illustrations
        // reserved space so their pixels can be painted over it afterwards.
        let mut lines: Vec<Line<'static>> = Vec::new();
        let mut pictures: Vec<Picture> = Vec::new();
        let mut anchor_line = None;
        let mut buttons = Vec::new();
        for entry in self.entries.iter_mut() {
            let start = lines.len();
            if self.pending_anchor == Some(entry.id) {
                anchor_line = Some(start);
            }
            if let Block::Image { path, .. } = &entry.block {
                buttons.push((start + 1, path.clone()));
            }
            let plan = entry.block.image_plan(area.width);
            lines.extend(entry.lines(area.width, tick).iter().cloned());
            if let Some(plan) = plan {
                pictures.push(Picture {
                    id: entry.id,
                    top: start + plan.skip_lines as usize,
                    plan,
                });
            }
        }
        self.total_lines = lines.len();

        let max = self.total_lines.saturating_sub(self.viewport);
        if let Some(start) = anchor_line {
            self.offset = start.min(max);
            self.follow = self.offset >= max;
        } else if self.follow {
            self.offset = max;
        } else {
            self.offset = self.offset.min(max);
        }
        self.pending_anchor = None;

        let button_width = crate::wrap::display_width(super::block::IMAGE_PREVIEW_BUTTON) as u16;
        self.image_buttons = buttons
            .into_iter()
            .filter_map(|(line, path)| {
                if line < self.offset || line >= self.offset + self.viewport {
                    return None;
                }
                Some((
                    Rect::new(
                        area.x + 2,
                        area.y + (line - self.offset) as u16,
                        button_width.min(area.width.saturating_sub(2)),
                        1,
                    ),
                    path,
                ))
            })
            .collect();

        let window: Vec<Line<'static>> = lines
            .into_iter()
            .skip(self.offset)
            .take(self.viewport)
            .collect();
        Paragraph::new(window).render(area, buf);
        self.render_pictures(area, buf, &pictures);
    }

    /// Paint illustrations over the rows their blocks reserved.
    ///
    /// Done after the text so the pixels are not overwritten, and clipped both
    /// ways: an image scrolled halfway off the top is drawn from the middle
    /// down, which is what makes scrolling past a picture look continuous.
    fn render_pictures(&mut self, area: Rect, buf: &mut Buffer, pictures: &[Picture]) {
        let first = self.offset;
        let last = self.offset + self.viewport;
        // Everything that scrolled out of view can drop its pixels.
        let live: std::collections::HashSet<u64> = pictures.iter().map(|p| p.id).collect();
        self.images.retain(|(id, _, _), _| live.contains(id));

        for picture in pictures {
            let bottom = picture.top + picture.plan.rows as usize;
            if bottom <= first || picture.top >= last {
                continue;
            }
            let skip = first.saturating_sub(picture.top) as u16;
            let y = area.y + (picture.top.saturating_sub(first)) as u16;
            let height = (bottom.min(last) - picture.top.max(first)) as u16;
            let target = Rect {
                x: area.x + 2,
                y,
                width: picture.plan.cols.min(area.width.saturating_sub(2)),
                height: height.min(area.height.saturating_sub(y - area.y)),
            };
            if target.width == 0 || target.height == 0 {
                continue;
            }

            if picture.plan.path.as_os_str().is_empty() {
                placeholder(target, buf, super::block::PLACEHOLDER_LABEL);
                continue;
            }

            let key = (picture.id, picture.plan.cols, picture.plan.rows);
            let decoded = self.images.entry(key).or_insert_with(|| {
                TermImage::open(&picture.plan.path, picture.plan.cols, picture.plan.rows).ok()
            });
            match decoded {
                Some(image) => {
                    image.render_clipped(target, buf, skip);
                }
                // Decoding failed after the header read succeeded: the frame is
                // the honest answer, not a blank hole.
                None => placeholder(target, buf, super::block::PLACEHOLDER_LABEL),
            }
        }
    }
}

/// An illustration placed in the laid-out frame.
struct Picture {
    id: u64,
    /// Index of its first pixel row among all laid-out lines.
    top: usize,
    plan: ImagePlan,
}

#[cfg(test)]
mod manual_viewport_tests {
    use super::*;

    fn screen(sb: &mut Scrollback) -> String {
        let area = Rect::new(0, 0, 48, 10);
        let mut buf = Buffer::empty(area);
        sb.render(area, &mut buf, 0);
        (0..area.height)
            .flat_map(|y| (0..area.width).map(move |x| (x, y)))
            .map(|(x, y)| buf[(x, y)].symbol().to_string())
            .collect()
    }

    #[test]
    fn long_passage_starts_at_its_opening_and_stays_put() {
        let mut sb = Scrollback::new();
        sb.push(Block::System("old material ".repeat(60)));
        screen(&mut sb);
        sb.begin_reading();
        let text = format!(
            "OPENING_SENTENCE {} ENDING_SENTENCE",
            "long paragraph content ".repeat(100)
        );
        sb.push(Block::passage_text(&text));
        sb.push(Block::System("completed".into()));
        let first = screen(&mut sb);
        assert!(first.contains("OPENING_SENTENCE"));
        assert!(!first.contains("ENDING_SENTENCE"));
        assert!(sb.scrolled_away());
        assert!(screen(&mut sb).contains("OPENING_SENTENCE"));
        sb.to_bottom();
        assert!(screen(&mut sb).contains("ENDING_SENTENCE"));
    }

    #[test]
    fn fitting_passage_remains_visible_with_following_status() {
        let mut sb = Scrollback::new();
        sb.begin_reading();
        sb.push(Block::passage_text("Complete short passage."));
        sb.push(Block::System("completed".into()));
        assert!(screen(&mut sb).contains("Complete short passage."));
        assert!(sb.follow);
        assert_eq!(sb.reading_window(), (48, 7));
    }
}

#[cfg(test)]
mod image_button_tests {
    use super::*;

    #[test]
    fn preview_buttons_follow_the_viewport_and_target_the_right_image() {
        let mut sb = Scrollback::new();
        sb.push(Block::System("old material ".repeat(100)));
        let first = std::path::PathBuf::from("/tmp/first.svg");
        let second = std::path::PathBuf::from("/tmp/second.png");
        sb.push(Block::image(first.clone(), "第一张图", 16));
        sb.push(Block::image(second.clone(), "第二张图", 16));
        let area = Rect::new(4, 3, 48, 10);
        let mut buf = Buffer::empty(Rect::new(0, 0, 60, 20));
        sb.render(area, &mut buf, 0);
        assert_eq!(sb.image_buttons.len(), 2);
        for (button, path) in &sb.image_buttons {
            assert_eq!(sb.image_at(button.x, button.y), Some(path.clone()));
            assert_eq!(sb.image_at(button.x - 1, button.y), None);
        }
        sb.to_top();
        sb.render(area, &mut buf, 0);
        assert!(
            sb.image_buttons.is_empty(),
            "offscreen images must not be clickable"
        );
        sb.clear();
        assert!(sb.image_buttons.is_empty());
    }
}
