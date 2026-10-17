use std::cell::RefCell;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};
use android_activity::AndroidApp;
use artcraft_launcher_core::protocol::{Backend, Capabilities, Command, Event};
use artcraft_launcher_core::{Arch, Os, Platform, SUITE};
use artcraft_launcher_ui_egui::{Host, LauncherUi, Settings};

struct AndroidHost;
impl Host for AndroidHost {
    fn reduce_motion(&self) -> bool {
        false
    }
}

struct AndroidLauncherApp {
    ui: LauncherUi,
}

impl eframe::App for AndroidLauncherApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ui.logic(ctx, &mut AndroidHost);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.ui.ui(ui, &mut AndroidHost);
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        self.ui.raw_input(ctx, raw_input);
    }
}

struct AndroidBackend {
    events: Rc<RefCell<Vec<Event>>>,
}

impl AndroidBackend {
    fn new(ctx: egui::Context) -> Self {
        let events = Rc::new(RefCell::new(Vec::new()));
        let backend = Self { events: events.clone() };

        for app in SUITE {
            events.borrow_mut().push(Event::Detected {
                app: app.id.into(),
                installed: None,
            });
        }
        ctx.request_repaint();

        backend
    }
}

impl Backend for AndroidBackend {
    fn poll(&mut self) -> Vec<Event> {
        self.events.borrow_mut().drain(..).collect()
    }

    fn send(&mut self, _command: Command) {}

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            manage_apps: true,
            reveal_label: "Open App",
            self_update: false,
        }
    }

    fn platform(&self) -> Option<Platform> {
        Some(Platform { os: Os::Linux, arch: Arch::Arm64 })
    }

    fn now(&self) -> i64 {
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
    }
}

#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("CraftLauncher"),
    );

    log::info!("Starting Craft Launcher on Huawei MatePad 12X (Android ARM64)...");

    let mut options = eframe::NativeOptions::default();
    options.android_app = Some(app);

    let _ = eframe::run_native(
        "Craft Launcher",
        options,
        Box::new(|cc| {
            let ctx = cc.egui_ctx.clone();
            let backend = AndroidBackend::new(ctx.clone());
            let settings = Settings::default();
            let ui = LauncherUi::new(&ctx, Box::new(backend), settings);
            Ok(Box::new(AndroidLauncherApp { ui }))
        }),
    );
}
