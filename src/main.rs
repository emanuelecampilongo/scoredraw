// SPDX-FileCopyrightText: 2026 Emanuele Campilongo
// SPDX-License-Identifier: GPL-3.0-only

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
#[macro_use]
mod language;
#[cfg(windows)]
mod audio;
#[cfg(windows)]
mod canvas;
#[cfg(windows)]
mod data;
#[cfg(windows)]
mod dialog;
#[cfg(windows)]
mod icons;
#[cfg(windows)]
mod ink;
#[cfg(windows)]
mod library;
#[cfg(windows)]
mod overlay;
#[cfg(windows)]
mod pdf;
#[cfg(windows)]
mod pdf_edit;
#[cfg(windows)]
mod preferences;
#[cfg(windows)]
mod radial;
#[cfg(windows)]
mod reader;
#[cfg(windows)]
mod render_queue;
#[cfg(windows)]
mod view;

#[cfg(windows)]
fn main() {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if let [flag, report, files @ ..] = arguments.as_slice()
        && flag == "--misura"
        && !files.is_empty()
    {
        let report = std::path::Path::new(report);
        let result = pdf::measure(report, files.iter().map(std::path::PathBuf::from));
        if let Err(error) = result {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    if let Err(error) = reader::run(
        arguments
            .into_iter()
            .map(std::path::PathBuf::from)
            .collect(),
    ) {
        reader::show_startup_error(&error);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("ScoreDraw requires Windows.");
}
