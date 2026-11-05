//! The game's patch notes, the worker's side. Both sources are asked for at start and on the idle
//! tick, each at most every 30 minutes counted from the last try, across restarts, with the ETag
//! of the copy kept (as SurvivorsForge's registry is polled). Every answer goes to this PC's cache
//! (`game_news::Cache`), which reads and remembers it; what the panel shows is the core's merge of
//! the two (`game_news::merge`). The network thread asks; nothing here waits for it. The window
//! only shows what is built here: nothing is parsed on the UI thread.

use super::Worker;
use super::view_model::{clock, when};
use crate::net::{Job, NewsAnswer};
use crate::view::{GameNewsView, GameNote};
use chrono::{DateTime, Local, Utc};
use satchel_core::changelog::{Block, Version};
use satchel_core::game_news::{self, Feed, Latest};
use satchel_core::log;
use std::path::PathBuf;

/// What the panel says under a version the file lacks, in place of its detail lines.
const ON_DISCORD: &str = "The full notes are on the game's Discord";
/// What the panel says after its 30 versions.
const OLDER: &str = "Older notes are in the game's Discord";
/// The panel with nothing cached at all.
const NOTHING_YET: &str = "No notes on this PC yet: they show once the game's update server or the notes file has answered.";

pub(super) struct NewsState {
    /// Both sources on this PC (`%LOCALAPPDATA%\SurvivorsSatchel\game-news`).
    cache: game_news::Cache,
    /// What this PC remembers, as of the cache's last change.
    state: game_news::State,
    /// The update index as last received.
    latest: Option<Latest>,
    /// The notes file as last received, as versions, newest first as the file has them.
    notes: Vec<Version>,
    /// Requests on their way.
    latest_out: bool,
    notes_out: bool,
}

impl NewsState {
    pub(super) fn new(dir: PathBuf) -> NewsState {
        NewsState {
            cache: game_news::Cache::open(dir),
            state: game_news::State::default(),
            latest: None,
            notes: Vec::new(),
            latest_out: false,
            notes_out: false,
        }
    }

    fn out(&mut self, feed: Feed) -> &mut bool {
        match feed {
            Feed::Latest => &mut self.latest_out,
            Feed::Notes => &mut self.notes_out,
        }
    }

    /// The cache's state again, after it changed.
    fn reread(&mut self) {
        self.state = self.cache.state();
    }
}

/// When a source last answered, for About: "14:40" today, else with its day.
fn answer_time(at: DateTime<Utc>) -> String {
    if at.with_timezone(&Local).date_naive() == Local::now().date_naive() { clock(at) } else { when(at) }
}

/// A request that brought nothing to keep, for the log.
fn failure(answer: &NewsAnswer) -> String {
    match answer {
        NewsAnswer::Body { .. } => "an answer".into(),
        NewsAnswer::NotModified => "nothing new (304) where a body was due".into(),
        NewsAnswer::Limited => "asked too often (429); the next try waits".into(),
        NewsAnswer::Down(why) => format!("no answer ({why})"),
    }
}

/// A block as the window's What's new shows it: 0 a paragraph, 1 a heading, 2 a point, 3 a point
/// inside a point.
fn block_line(block: &Block) -> (i32, String) {
    match block {
        Block::Paragraph(t) => (0, t.clone()),
        Block::Heading(t) => (1, t.clone()),
        Block::Bullet(t) => (2, t.clone()),
        Block::Inner(t) => (3, t.clone()),
    }
}

impl Worker {
    /// What this PC's cache holds, read at start: both sources as last received.
    pub(super) fn news_load(&mut self) {
        let n = &mut self.news;
        n.latest = n.cache.latest();
        n.notes = n.cache.notes().unwrap_or_default();
        n.reread();
    }

    /// Each source when it's due: at most every 30 minutes, counted from the last try, across
    /// restarts (`game_news::due`), with the ETag of the copy kept. Called at every tick, so it
    /// costs nothing until one is due (the state in memory follows every change).
    pub(super) fn news_look(&mut self) {
        let now = Utc::now();
        for feed in [Feed::Latest, Feed::Notes] {
            if *self.news.out(feed) || !game_news::due(self.news.state.source(feed), now) {
                continue;
            }
            // The ETag only while the copy it names is still on disk.
            self.news.reread();
            let etag = self.news.state.source(feed).etag.clone();
            let sent = self.net.send(Job::GameNews { feed, etag }).is_ok();
            *self.news.out(feed) = sent;
        }
    }

    /// A source's answer, kept with its bookkeeping: a 304 keeps what is there, and anything but
    /// an answer counts as a try (the next one waits its 30 minutes).
    pub(super) fn news_answer(&mut self, feed: Feed, answer: NewsAnswer) {
        let now = Utc::now();
        let n = &mut self.news;
        *n.out(feed) = false;
        match answer {
            NewsAnswer::Body { text, etag, modified } => match feed {
                Feed::Latest => {
                    let modified = modified.as_deref().and_then(game_news::http_date);
                    match n.cache.store_latest(&text, etag.as_deref(), modified, now) {
                        Ok(latest) => {
                            log!("the game's update server: build {} is current", latest.version);
                            n.latest = Some(latest);
                        }
                        Err(why) => log!("{why}"),
                    }
                }
                Feed::Notes => match n.cache.store_notes(&text, etag.as_deref(), now) {
                    Ok(versions) => {
                        log!("the game's patch notes: {} versions in the file", versions.len());
                        n.notes = versions;
                    }
                    Err(why) => log!("{why}"),
                },
            },
            NewsAnswer::NotModified => n.cache.touch(feed, now),
            other => {
                log!("{}: {}", feed.name(), failure(&other));
                n.cache.tried(feed, now);
            }
        }
        n.reread();
    }

    /// The panel was opened: the Play line stops being new for the version it names.
    pub(super) fn news_seen(&mut self) {
        if let Some(version) = self.news.latest.as_ref().map(|l| l.version.clone()) {
            self.news.cache.opened(&version);
            self.news.reread();
        }
    }

    /// The Play line, the panel's versions and About's two answer times.
    pub(super) fn news_view(&self) -> GameNewsView {
        let n = &self.news;
        let version = n.latest.as_ref().map(|l| l.version.clone()).unwrap_or_default();
        let merged = game_news::merge(&n.notes, &n.state.seen);
        let (shown, more) = game_news::panel_notes(&merged);
        let notes: Vec<GameNote> = shown
            .iter()
            .map(|note| {
                let mut blocks: Vec<(i32, String)> = Vec::new();
                if !note.paragraph.is_empty() {
                    blocks.push((0, note.paragraph.clone()));
                }
                if note.in_file && !note.blocks.is_empty() {
                    blocks.extend(note.blocks.iter().map(block_line));
                } else {
                    blocks.push((4, ON_DISCORD.into()));
                }
                GameNote { version: note.version.clone(), date: note.date.map(|d| d.format("%-d %b %Y").to_string()).unwrap_or_default(), blocks }
            })
            .collect();
        let answer = |feed: Feed| match n.state.source(feed).answered {
            Some(at) => format!("last answer {}", answer_time(at)),
            None => "no answer yet on this PC".to_string(),
        };
        GameNewsView {
            new: game_news::newest_is_new(&n.state, &version),
            version,
            footer: if more { OLDER.into() } else { String::new() },
            empty: if notes.is_empty() { NOTHING_YET.into() } else { String::new() },
            notes,
            latest_answer: answer(Feed::Latest),
            notes_answer: answer(Feed::Notes),
        }
    }
}
