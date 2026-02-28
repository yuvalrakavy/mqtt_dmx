mod config;
mod state;
mod udp;
mod ui;

use dioxus::prelude::*;

static CSS: Asset = asset!("/assets/main.css");

fn main() {
    #[cfg(feature = "server")]
    {
        use rustop::opts;

        let (args, _rest) = opts! {
            synopsis "ArtNet DMX Emulator";
            opt config_path: Option<String>, desc: "Path to config JSON file", long: "config", short: 'c';
            opt port: u16 = 6454, desc: "ArtNet UDP port", long: "port", short: 'p';
            opt web_port: u16 = 8080, desc: "Port for the web UI", long: "web-port", short: 'w';
        }
        .parse_or_exit();

        println!("Starting ArtNet Emulator — UDP port: {}, Web port: {}", args.port, args.web_port);

        if std::env::var("PORT").is_err() {
            unsafe { std::env::set_var("PORT", args.web_port.to_string()) };
        }
        if std::env::var("IP").is_err() {
            unsafe { std::env::set_var("IP", "0.0.0.0") };
        }

        dioxus::launch(App);
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
            p { "Emulator starting..." }
        }
    }
}
