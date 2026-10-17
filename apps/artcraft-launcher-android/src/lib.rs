use android_activity::AndroidApp;
use artcraft_launcher_core::protocol::{Backend, Capabilities, Command, Event, LatestRelease, Origin, Source};
use artcraft_launcher_core::status::{InstallSource, Installed};
use artcraft_launcher_core::version;
use artcraft_launcher_core::{Arch, Os, Platform, SUITE};
use artcraft_launcher_ui_egui::{Host, LauncherUi, Settings};
use jni::objects::JValue;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

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

    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}
}

struct AndroidBackend {
    app: AndroidApp,
    ctx: egui::Context,
    events: Rc<RefCell<Vec<Event>>>,
}

impl AndroidBackend {
    fn new(app: AndroidApp, ctx: egui::Context) -> Self {
        let events = Rc::new(RefCell::new(Vec::new()));
        Self { app, ctx, events }
    }

    fn rescan(&mut self) {
        let now_sec = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        for app in SUITE {
            let pkg = format!("ai.storyteller.{}", app.id);
            let is_installed = is_package_installed(&self.app, &pkg);
            log::info!("Check package {}: installed={}", pkg, is_installed);

            let installed = if is_installed {
                Some(Installed {
                    version: version::parse("0.9.2"),
                    source: InstallSource::Launcher,
                    location: format!("/data/app/{}", pkg),
                    package: Some(pkg.clone()),
                })
            } else {
                None
            };

            self.events.borrow_mut().push(Event::Detected {
                app: app.id.into(),
                installed,
            });

            let v_str = if is_installed { "0.9.2" } else { "0.1.0" };
            let release = artcraft_launcher_core::release::Release {
                tag: format!("v{v_str}-android"),
                version: version::parse(v_str),
                name: format!("{} v{}", app.name, v_str),
                published_at: Some(now_sec),
                html_url: format!("https://github.com/Lennowlen/{}/releases", app.id),
                notes: "Android ARM64 native release for Huawei MatePad 12X".into(),
                prerelease: false,
                assets: vec![artcraft_launcher_core::release::Asset {
                    name: format!("{}-{}-linux-aarch64.AppImage", app.id, v_str),
                    size: 45_000_000,
                    url: format!(
                        "https://github.com/Lennowlen/{}/releases/download/v{}-android/{}.apk",
                        app.id, v_str, app.id
                    ),
                    sha256: None,
                }],
            };

            self.events.borrow_mut().push(Event::Latest {
                app: app.id.into(),
                latest: LatestRelease::Found(release),
                from: Origin::GitHub,
            });
        }

        self.events.borrow_mut().push(Event::CheckFinished {
            at: now_sec,
            source: Source::Feed,
            rate_limited_until: None,
        });

        self.ctx.request_repaint();
    }
}

impl Backend for AndroidBackend {
    fn poll(&mut self) -> Vec<Event> {
        self.events.borrow_mut().drain(..).collect()
    }

    fn send(&mut self, command: Command) {
        match command {
            Command::Launch { app } => {
                let pkg = format!("ai.storyteller.{}", app);
                log::info!("Launching package: {}", pkg);
                launch_package(&self.app, &pkg);
            }
            Command::Install { app } => {
                log::info!("Install command requested for: {}", app);
                let pkg = format!("ai.storyteller.{}", app);
                launch_package(&self.app, &pkg);
            }
            Command::Detect | Command::CheckForUpdates { .. } => {
                log::info!("Re-scanning installed apps...");
                self.rescan();
            }
            _ => {}
        }
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            manage_apps: true,
            reveal_label: "Open App",
            self_update: false,
        }
    }

    fn platform(&self) -> Option<Platform> {
        Some(Platform {
            os: Os::Linux,
            arch: Arch::Arm64,
        })
    }

    fn now(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
}

fn is_package_installed(app: &AndroidApp, package_name: &str) -> bool {
    let vm_ptr = app.vm_as_ptr();
    let activity_ptr = app.activity_as_ptr();
    if vm_ptr.is_null() || activity_ptr.is_null() {
        return false;
    }

    unsafe {
        let jvm = match jni::JavaVM::from_raw(vm_ptr as *mut jni::sys::JavaVM) {
            Ok(jvm) => jvm,
            Err(_) => return false,
        };
        let mut env = match jvm.attach_current_thread() {
            Ok(env) => env,
            Err(_) => return false,
        };
        let activity = match jni::objects::JObject::from_raw(activity_ptr as jni::sys::jobject) {
            activity if !activity.as_raw().is_null() => activity,
            _ => return false,
        };

        // Context.getPackageManager()
        let pm_val = match env.call_method(&activity, "getPackageManager", "()Landroid/content/pm/PackageManager;", &[]) {
            Ok(val) => val,
            Err(e) => {
                log::warn!("getPackageManager failed: {:?}", e);
                return false;
            }
        };
        let pm = match pm_val.l() {
            Ok(pm) if !pm.as_raw().is_null() => pm,
            _ => return false,
        };

        let pkg_jstring = match env.new_string(package_name) {
            Ok(s) => s,
            Err(_) => return false,
        };

        // First try: getLaunchIntentForPackage(package_name) != null
        if let Ok(intent_val) = env.call_method(&pm, "getLaunchIntentForPackage", "(Ljava/lang/String;)Landroid/content/Intent;", &[JValue::Object(&pkg_jstring)]) {
            if let Ok(intent_obj) = intent_val.l() {
                if !intent_obj.as_raw().is_null() {
                    log::info!("Package {} found via getLaunchIntentForPackage", package_name);
                    return true;
                }
            }
        }
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
        }

        // Second try: getPackageInfo(package_name, 0)
        let res = env.call_method(&pm, "getPackageInfo", "(Ljava/lang/String;I)Landroid/content/pm/PackageInfo;", &[JValue::Object(&pkg_jstring), JValue::Int(0)]);
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
            return false;
        }

        let is_ok = res.is_ok();
        log::info!("Package {} getPackageInfo result: {}", package_name, is_ok);
        is_ok
    }
}

fn launch_package(app: &AndroidApp, package_name: &str) {
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

        // Context.getPackageManager()
        let pm_val = match env.call_method(&activity, "getPackageManager", "()Landroid/content/pm/PackageManager;", &[]) {
            Ok(val) => val,
            Err(_) => return,
        };
        let pm = match pm_val.l() {
            Ok(pm) if !pm.as_raw().is_null() => pm,
            _ => return,
        };

        let pkg_jstring = match env.new_string(package_name) {
            Ok(s) => s,
            Err(_) => return,
        };

        // PackageManager.getLaunchIntentForPackage(String) -> Intent
        let intent_val = match env.call_method(&pm, "getLaunchIntentForPackage", "(Ljava/lang/String;)Landroid/content/Intent;", &[JValue::Object(&pkg_jstring)]) {
            Ok(val) => val,
            Err(_) => return,
        };
        let intent = match intent_val.l() {
            Ok(i) if !i.as_raw().is_null() => i,
            _ => {
                log::warn!("Could not get launch intent for package {}", package_name);
                return;
            }
        };

        // Activity.startActivity(Intent)
        let _ = env.call_method(&activity, "startActivity", "(Landroid/content/Intent;)V", &[JValue::Object(&intent)]);
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
        }
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

    log::info!("artcraft_launcher_android: Starting Craft Launcher on Huawei MatePad 12X (Android ARM64)...");

    apply_immersive_sticky(&app);

    let app_clone = app.clone();
    let mut options = eframe::NativeOptions::default();
    options.android_app = Some(app);

    let _ = eframe::run_native(
        "Craft Launcher",
        options,
        Box::new(move |cc| {
            let mut backend = AndroidBackend::new(app_clone, cc.egui_ctx.clone());
            backend.rescan();

            let ui = LauncherUi::new(&cc.egui_ctx, Box::new(backend), Settings::default());
            Ok(Box::new(AndroidLauncherApp { ui }))
        }),
    );
}
