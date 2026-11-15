#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use eframe::egui;
use nip_native::{audio::AudioEngine, contracts::Command, network, ui::NipApp};
use std::{
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};

fn launch_options(
    args: impl Iterator<Item = String>,
) -> Result<(PathBuf, Option<Duration>, bool), String> {
    let mut profile = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("NIPNative");
    let mut quit_after = None;
    let mut local_only = false;
    let mut args = args;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--local-only" => local_only = true,
            "--data-dir" => {
                profile = args
                    .next()
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from)
                    .ok_or("Falta una carpeta después de --data-dir")?
            }
            "--quit-after" => {
                let seconds: u64 = args
                    .next()
                    .ok_or("Faltan segundos después de --quit-after")?
                    .parse()
                    .map_err(|_| "--quit-after requiere segundos enteros")?;
                quit_after = Some(Duration::from_secs(seconds));
            }
            _ => return Err(format!("Opción desconocida: {arg}")),
        }
    }
    Ok((profile, quit_after, local_only))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (profile, quit_after, local_only) = launch_options(std::env::args().skip(1))?;
    let (events_tx, events_rx) = mpsc::channel();
    let (audio, frames) = AudioEngine::spawn(events_tx.clone(), Default::default())?;
    let commands = if local_only {
        network::spawn_loopback(profile, events_tx, audio, frames)
    } else {
        network::spawn(profile, events_tx, audio, frames)
    };
    let mut app = NipApp::new(commands.clone(), events_rx);
    app.quit_at = quit_after.map(|delay| Instant::now() + delay);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(if local_only {
                "NIP · QA local"
            } else {
                "NIP · Nativo"
            })
            .with_icon(eframe::icon_data::from_png_bytes(include_bytes!(
                "../assets/nip-cat-256.png"
            ))?)
            .with_inner_size([1080.0, 720.0])
            .with_min_inner_size([760.0, 540.0]),
        ..Default::default()
    };
    let result = eframe::run_native(
        "NIP · Nativo",
        options,
        Box::new(move |cc| {
            nip_native::ui::configure(&cc.egui_ctx);
            match nip_native::desktop::Desktop::new(&cc.egui_ctx) {
                Ok(desktop) => app.desktop = Some(desktop),
                Err(error) => eprintln!("No se pudo crear el icono de bandeja: {error}"),
            }
            Ok(Box::new(app))
        }),
    );
    let _ = commands.send(Command::Shutdown);
    result.map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn isolated_profile_and_bounded_development_run() {
        let (profile, quit, local_only) = launch_options(
            [
                "--data-dir",
                "C:/isolated/nip",
                "--quit-after",
                "3",
                "--local-only",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();
        assert_eq!(profile, PathBuf::from("C:/isolated/nip"));
        assert_eq!(quit, Some(Duration::from_secs(3)));
        assert!(local_only);
        assert!(!launch_options(std::iter::empty()).unwrap().2);
        assert!(launch_options(["--data-dir"].into_iter().map(String::from)).is_err());
        assert!(launch_options(["--quit-after", "oops"].into_iter().map(String::from)).is_err());
    }
}
