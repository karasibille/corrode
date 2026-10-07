//! `corrode`: cull the shots of a directory in the terminal.
//!
//! Shots are shown one at a time, with a strip of the burst they belong
//! to. Marks are written to RawTherapee sidecars as soon as they are set.

mod app;
mod culling;
mod encoder;
mod loader;
mod text;
mod viewer;

use std::env;
use std::error::Error;
use std::path::PathBuf;
use std::time::Duration;

use corrode_core::pairing;
use corrode_core::rawtherapee::Config;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui_image::picker::Picker;

use viewer::Viewer;

fn main() -> Result<(), Box<dyn Error>> {
    let dir = env::args()
        .nth(1)
        .map_or_else(|| PathBuf::from("."), PathBuf::from);
    let shots = pairing::scan_dir(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    if shots.is_empty() {
        return Err(format!("{}: no JPEG or RAW file", dir.display()).into());
    }

    let config = Config::load().map_err(|err| err.to_string());
    let mut terminal = ratatui::init();
    let picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
    let mut viewer = Viewer::new(dir, shots, picker, config);

    let result = (|| -> Result<(), Box<dyn Error>> {
        while !viewer.app.quit {
            viewer.schedule();
            viewer.receive();
            terminal.draw(|frame| viewer.draw(frame))?;
            if event::poll(Duration::from_millis(20))?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                viewer.key(key);
            }
        }
        Ok(())
    })();

    ratatui::restore();
    viewer.save_cache();
    println!("{}", viewer.summary());
    result
}
