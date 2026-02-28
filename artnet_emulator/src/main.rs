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
        #[allow(unused_mut)]
        let mut emulator_state = if let Some(config_path) = &args.config_path {
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
    rsx! {
        document::Stylesheet { href: CSS }
        div { class: "min-h-screen bg-gray-900 text-gray-100 p-4",
            h1 { class: "text-2xl font-bold mb-4", "ArtNet DMX Emulator" }
            p { "Listening for ArtNet packets..." }
        }
    }
}
