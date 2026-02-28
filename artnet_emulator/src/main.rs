mod config;
mod state;
mod udp;
mod ui;

use dioxus::prelude::*;

static CSS: Asset = asset!("/assets/main.css");

fn main() {
    #[cfg(feature = "server")]
    {
        use std::sync::Arc;
        use tokio::sync::RwLock;

        use dioxus::server::{DioxusRouterExt, ServeConfig};
        use rustop::opts;

        use crate::state::StateVersionNotifier;
        use crate::udp::log::SharedLog;

        let (args, _rest) = opts! {
            synopsis "ArtNet DMX Emulator";
            opt config_path: Option<String>, desc: "Path to config JSON file", long: "config", short: 'c';
            opt port: u16 = 6454, desc: "ArtNet UDP port", long: "port", short: 'p';
            opt web_port: u16 = 8080, desc: "Port for the web UI", long: "web-port", short: 'w';
        }
        .parse_or_exit();

        // Load config or start with empty state
        let emulator_state = if let Some(config_path) = &args.config_path {
            match std::fs::read_to_string(config_path) {
                Ok(json) => match serde_json::from_str::<config::Config>(&json) {
                    Ok(cfg) => {
                        if let Err(e) = cfg.validate() {
                            eprintln!("Config validation error: {e}");
                            std::process::exit(1);
                        }
                        let mut state = cfg.to_emulator_state();
                        state.config_path = Some(config_path.clone());
                        println!(
                            "Loaded config '{}': {} universes",
                            config_path,
                            state.universes.len()
                        );
                        state
                    }
                    Err(e) => {
                        eprintln!("Error parsing config '{config_path}': {e}");
                        std::process::exit(1);
                    }
                },
                Err(e) => {
                    eprintln!("Error reading config '{config_path}': {e}");
                    std::process::exit(1);
                }
            }
        } else {
            println!("No config file — running in auto-discovery mode");
            state::EmulatorState::empty()
        };

        let shared_state = Arc::new(RwLock::new(emulator_state));
        let shared_log = SharedLog::new(500);
        let state_version = StateVersionNotifier::new();

        let udp_port = args.port;
        let web_port = args.web_port;

        println!("Starting ArtNet Emulator — UDP port: {udp_port}, Web port: {web_port}");

        if std::env::var("PORT").is_err() {
            unsafe { std::env::set_var("PORT", web_port.to_string()) };
        }
        if std::env::var("IP").is_err() {
            unsafe { std::env::set_var("IP", "0.0.0.0") };
        }

        dioxus::serve(move || {
            let shared_state = shared_state.clone();
            let shared_log = shared_log.clone();
            let state_version = state_version.clone();

            async move {
                let bind_addr = format!("0.0.0.0:{udp_port}");
                udp::server::start_server(
                    &bind_addr,
                    shared_state.clone(),
                    shared_log.clone(),
                    state_version.clone(),
                )
                .await;

                let cfg = ServeConfig::new();
                let router = dioxus::server::axum::Router::new()
                    .serve_dioxus_application(cfg, App)
                    .layer(dioxus::server::axum::Extension(shared_state))
                    .layer(dioxus::server::axum::Extension(shared_log))
                    .layer(dioxus::server::axum::Extension(state_version));
                Ok(router)
            }
        });
    }

    #[cfg(not(feature = "server"))]
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    let mut snapshot = use_signal(|| None::<Result<ui::snapshot::EmulatorSnapshot, ServerFnError>>);

    use_future(move || async move {
        let result = get_snapshot().await;
        snapshot.set(Some(result));

        let mut last_version = 0u64;
        loop {
            match wait_for_update(last_version).await {
                Ok((version, snap)) => {
                    last_version = version;
                    snapshot.set(Some(Ok(snap)));
                }
                Err(_) => {
                    // On error, re-fetch snapshot directly and retry
                    let result = get_snapshot().await;
                    snapshot.set(Some(result));
                }
            }
        }
    });

    match snapshot() {
        Some(Ok(snap)) => {
            let config_label = snap.config_path.as_deref().unwrap_or("Auto-discovery mode");
            let universe_count = snap.universes.len();
            rsx! {
                document::Stylesheet { href: CSS }
                div { class: "min-h-screen bg-gray-900 text-gray-100 p-4",
                    h1 { class: "text-2xl font-bold mb-4", "ArtNet DMX Emulator" }
                    p { class: "text-gray-500 text-sm mb-4", "{config_label} — {universe_count} universes" }

                    if snap.universes.is_empty() {
                        p { class: "text-gray-400 italic", "Waiting for ArtNet packets..." }
                    }

                    for universe in &snap.universes {
                        div { class: "mb-6 bg-gray-800 rounded-lg p-4",
                            h2 { class: "text-lg font-semibold mb-2",
                                "{universe.label}"
                                if let Some(desc) = &universe.description {
                                    span { class: "text-gray-400 text-sm ml-2", "({desc})" }
                                }
                                span { class: "text-gray-500 text-sm ml-2", "— {universe.packet_count} packets" }
                            }

                            // Light view
                            h3 { class: "text-sm font-semibold text-gray-400 mb-2", "Lights" }
                            div { class: "grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-6 gap-2 mb-4",
                                for light in &universe.lights {
                                    {render_light(light)}
                                }
                            }

                            // Raw channel view
                            h3 { class: "text-sm font-semibold text-gray-400 mb-2", "Channels" }
                            div { class: "grid grid-cols-16 gap-px text-xs",
                                for (i, &value) in universe.channels.iter().enumerate() {
                                    {render_channel(i + 1, value)}
                                }
                            }
                        }
                    }
                }
            }
        }
        Some(Err(e)) => rsx! {
            document::Stylesheet { href: CSS }
            div { class: "min-h-screen bg-gray-900 text-red-400 p-4",
                h1 { class: "text-2xl font-bold mb-4", "ArtNet DMX Emulator" }
                p { "Error: {e}" }
            }
        },
        None => rsx! {
            document::Stylesheet { href: CSS }
            div { class: "min-h-screen bg-gray-900 text-gray-400 p-4",
                h1 { class: "text-2xl font-bold mb-4", "ArtNet DMX Emulator" }
                p { "Loading..." }
            }
        },
    }
}

fn render_light(light: &ui::snapshot::LightSnapshot) -> Element {
    use ui::snapshot::LightTypeSnapshot;

    match &light.light_type {
        LightTypeSnapshot::Rgb { r, g, b, .. } => {
            let color = format!("rgb({r},{g},{b})");
            rsx! {
                div { class: "bg-gray-700 rounded p-2 text-center",
                    div {
                        class: "w-full h-8 rounded mb-1",
                        style: "background-color: {color}",
                    }
                    span { class: "text-xs", "{light.name}" }
                    span { class: "text-xs text-gray-400 block", "({r},{g},{b})" }
                }
            }
        }
        LightTypeSnapshot::TriWhite { w1, w2, w3, .. } => {
            let avg = ((*w1 as u16 + *w2 as u16 + *w3 as u16) / 3) as u8;
            let color = format!("rgb({avg},{avg},{avg})");
            rsx! {
                div { class: "bg-gray-700 rounded p-2 text-center",
                    div {
                        class: "w-full h-8 rounded mb-1",
                        style: "background-color: {color}",
                    }
                    span { class: "text-xs", "{light.name}" }
                    span { class: "text-xs text-gray-400 block", "({w1},{w2},{w3})" }
                }
            }
        }
        LightTypeSnapshot::Single { value, .. } => {
            let color = format!("rgb({value},{value},{value})");
            rsx! {
                div { class: "bg-gray-700 rounded p-2 text-center",
                    div {
                        class: "w-full h-8 rounded mb-1",
                        style: "background-color: {color}",
                    }
                    span { class: "text-xs", "{light.name}" }
                    span { class: "text-xs text-gray-400 block", "{value}" }
                }
            }
        }
    }
}

fn render_channel(number: usize, value: u8) -> Element {
    let bg = if value > 0 {
        format!("rgba(59, 130, 246, {})", value as f32 / 255.0)
    } else {
        "transparent".to_string()
    };

    rsx! {
        div {
            class: "w-full h-6 text-center leading-6 border border-gray-700 text-xs",
            style: "background-color: {bg}",
            title: "Channel {number}: {value}",
            "{value}"
        }
    }
}

#[server]
async fn get_snapshot() -> Result<ui::snapshot::EmulatorSnapshot, ServerFnError> {
    use dioxus::fullstack::FullstackContext;
    use dioxus::server::axum::Extension;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    let Extension(state): Extension<Arc<RwLock<state::EmulatorState>>> =
        FullstackContext::extract().await?;
    let s = state.read().await;
    Ok(ui::snapshot::build_snapshot(&s))
}

#[server]
async fn wait_for_update(
    last_version: u64,
) -> Result<(u64, ui::snapshot::EmulatorSnapshot), ServerFnError> {
    use dioxus::fullstack::FullstackContext;
    use dioxus::server::axum::Extension;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    let Extension(version_notifier): Extension<state::StateVersionNotifier> =
        FullstackContext::extract().await?;
    let Extension(state): Extension<Arc<RwLock<state::EmulatorState>>> =
        FullstackContext::extract().await?;

    let mut rx = version_notifier.subscribe();
    loop {
        if *rx.borrow() != last_version {
            break;
        }
        if rx.changed().await.is_err() {
            break;
        }
    }
    let version = *rx.borrow();

    let s = state.read().await;
    Ok((version, ui::snapshot::build_snapshot(&s)))
}

#[server]
async fn get_log_entries(
    last_version: u64,
) -> Result<(u64, std::collections::VecDeque<udp::log::LogEntry>), ServerFnError> {
    use dioxus::fullstack::FullstackContext;
    use dioxus::server::axum::Extension;

    let Extension(shared_log): Extension<udp::log::SharedLog> =
        FullstackContext::extract().await?;
    Ok(shared_log.wait_and_get(last_version).await)
}
