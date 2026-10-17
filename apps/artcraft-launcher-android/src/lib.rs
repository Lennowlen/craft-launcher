use std::cell::RefCell;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};
use android_activity::AndroidApp;
use artcraft_launcher_core::protocol::{Backend, Capabilities, Command, Event, LatestRelease, Origin, Source};
use artcraft_launcher_core::status::{InstallSource, Installed};
use artcraft_launcher_core::version;
use artcraft_launcher_core::{Arch, Os, Platform, SUITE};
use artcraft_launcher_ui_egui::{Host, LauncherUi, Settings};
use jni::objects::JValue;

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

        let now_sec = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);

        for app in SUITE {
            let is_installed = app.id == "photocraft";
            let installed = if is_installed {
                Some(Installed {
                    version: version::parse("0.9.2"),
                    source: InstallSource::Launcher,
                    location: "/data/app/ai.storyteller.photocraft".into(),
                    package: Some("ai.storyteller.photocraft".into()),
                })
            } else {
                None
            };

            // 1. Laporkan status deteksi modul
            events.borrow_mut().push(Event::Detected {
                app: app.id.into(),
                installed,
            });

            // 2. Berikan metadata rilis agar status berubah dari "Checking..." menjadi "Up to date" atau "Coming soon"
            let v_str = if is_installed { "0.9.2" } else { "0.1.0" };
            let release = artcraft_launcher_core::release::Release {
                tag: format!("v{v_str}-android"),
                version: version::parse(v_str),
                name: format!("{} v{}", app.name, v_str),
                published_at: Some(now_sec),
                html_url: format!("https://github.com/Lennowlen/{}/releases", app.id),
                notes: "Android ARM64 native release for Huawei MatePad 12X".into(),
                prerelease: false,
                assets: vec![
                    artcraft_launcher_core::release::Asset {
                        name: format!("{}-{}-linux-aarch64.AppImage", app.id, v_str),
                        size: 45_000_000,
                        url: format!("https://github.com/Lennowlen/{}/releases/download/v{}-android/{}.apk", app.id, v_str, app.id),
                        sha256: None,
                    }
                ],
            };

            events.borrow_mut().push(Event::Latest {
                app: app.id.into(),
                latest: LatestRelease::Found(release),
                from: Origin::GitHub,
            });
        }

        // Tuntaskan status check updates global
        events.borrow_mut().push(Event::CheckFinished {
            at: now_sec,
            source: Source::Feed,
            rate_limited_until: None,
        });

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

fn apply_immersive_sticky(app: &AndroidApp) {
    let vm_ptr = app.vm_as_ptr();
    let activity_ptr = app.activity_as_ptr();
    if vm_ptr.is_null() || activity_ptr.is_null() {
        return;
    }

    unsafe {
        let jvm = match jni::JavaVM::from_raw(vm_ptr as *mut jni::sys::JavaVM) {
            Ok(jvm) => jvm,
            Err(_) => return,
        };
        let mut env = match jvm.attach_current_thread() {
            Ok(env) => env,
            Err(_) => return,
        };
        let activity = match jni::objects::JObject::from_raw(activity_ptr as jni::sys::jobject) {
            activity if !activity.as_raw().is_null() => activity,
            _ => return,
        };

        if let Ok(window) = env.call_method(&activity, "getWindow", "()Landroid/view/Window;", &[]) {
            if let Ok(window_obj) = window.l() {
                if let Ok(decor_view) = env.call_method(&window_obj, "getDecorView", "()Landroid/view/View;", &[]) {
                    if let Ok(decor_obj) = decor_view.l() {
                        // SYSTEM_UI_FLAG_IMMERSIVE_STICKY (4096)
                        // | SYSTEM_UI_FLAG_FULLSCREEN (4)
                        // | SYSTEM_UI_FLAG_HIDE_NAVIGATION (2)
                        // | SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN (1024)
                        // | SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION (512)
                        // | SYSTEM_UI_FLAG_LAYOUT_STABLE (256)
                        let flags: i32 = 4096 | 4 | 2 | 1024 | 512 | 256;
                        let _ = env.call_method(&decor_obj, "setSystemUiVisibility", "(I)V", &[JValue::Int(flags)]);
                    }
                }
            }
        }
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

    apply_immersive_sticky(&app);

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
