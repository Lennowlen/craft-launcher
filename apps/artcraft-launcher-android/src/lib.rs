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

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.ui.ui(ui, &mut AndroidHost);
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        self.ui.raw_input(ctx, raw_input);
    }
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
            let installed_vname = if is_installed {
                get_package_version_name(&self.app, &pkg)
            } else {
                None
            };
            log::info!("Check package {}: installed={}, version={:?}", pkg, is_installed, installed_vname);

            let installed = if is_installed {
                let v = installed_vname
                    .as_deref()
                    .and_then(version::parse)
                    .or_else(|| match app.id {
                        "photocraft" => version::parse("0.9.2"),
                        "cadcraft" => version::parse("0.7.0"),
                        "vectorcraft" => version::parse("0.12.0"),
                        _ => version::parse("0.1.0"),
                    });
                Some(Installed {
                    version: v,
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

            // Dual version mapping: default fork release version vs installed version
            let latest_v_str = match app.id {
                "photocraft" => "0.9.3", // Menawarkan update jika installed masih 0.9.2
                "cadcraft" => "0.7.0",
                "vectorcraft" => "0.12.0",
                _ => "0.1.0",
            };
            let release = artcraft_launcher_core::release::Release {
                tag: format!("v{latest_v_str}-android"),
                version: version::parse(latest_v_str),
                name: format!("{} v{} (Mobile Port)", app.name, latest_v_str),
                published_at: Some(now_sec),
                html_url: format!("https://github.com/Lennowlen/{}/releases", app.id),
                notes: format!("ArtCraft Suite Android ARM64 port by Lennowlen for Huawei MatePad 12X (Upstream base vs Fork v{})", latest_v_str),
                prerelease: false,
                assets: vec![artcraft_launcher_core::release::Asset {
                    name: format!("{}-{}.apk", app.id, latest_v_str),
                    size: 45_000_000,
                    url: format!(
                        "https://github.com/Lennowlen/{}/releases/download/v{}-android/{}.apk",
                        app.id, latest_v_str, app.id
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

        // Self-update launcher check
        let launcher_installed_v = get_package_version_name(&self.app, "ai.storyteller.craftlauncher")
            .as_deref()
            .and_then(version::parse)
            .unwrap_or_else(|| version::parse("0.4.2").unwrap());
        let launcher_latest_v = version::parse("0.4.3").unwrap();
        if version::is_newer(&launcher_latest_v, &launcher_installed_v) {
            let launcher_release = artcraft_launcher_core::release::Release {
                tag: "v0.4.3-android".into(),
                version: Some(launcher_latest_v),
                name: "Craft Launcher v0.4.3".into(),
                published_at: Some(now_sec),
                html_url: "https://github.com/Lennowlen/craft-launcher/releases".into(),
                notes: "Pembaruan Craft Launcher ARM64: Dukungan dual-version tracking dan update installer.".into(),
                prerelease: false,
                assets: vec![artcraft_launcher_core::release::Asset {
                    name: "CraftLauncher.apk".into(),
                    size: 7_500_000,
                    url: "https://github.com/Lennowlen/craft-launcher/releases/download/v0.4.3-android/CraftLauncher.apk".into(),
                    sha256: None,
                }],
            };
            self.events.borrow_mut().push(Event::LauncherUpdate(launcher_release));
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
                install_module(&self.app, &app);
            }
            Command::Uninstall { app } => {
                log::info!("Uninstall command requested for: {}", app);
                let pkg = format!("ai.storyteller.{}", app);
                uninstall_package(&self.app, &pkg);
            }
            Command::UpdateLauncher => {
                log::info!("UpdateLauncher command requested");
                install_module(&self.app, "craft-launcher");
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
            self_update: true,
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

fn get_package_version_name(app: &AndroidApp, package_name: &str) -> Option<String> {
    let vm_ptr = app.vm_as_ptr();
    let activity_ptr = app.activity_as_ptr();
    if vm_ptr.is_null() || activity_ptr.is_null() {
        return None;
    }

    unsafe {
        let jvm = match jni::JavaVM::from_raw(vm_ptr as *mut jni::sys::JavaVM) {
            Ok(jvm) => jvm,
            Err(_) => return None,
        };
        let mut env = match jvm.attach_current_thread() {
            Ok(env) => env,
            Err(_) => return None,
        };
        let activity = match jni::objects::JObject::from_raw(activity_ptr as jni::sys::jobject) {
            activity if !activity.as_raw().is_null() => activity,
            _ => return None,
        };

        let pm_val = match env.call_method(&activity, "getPackageManager", "()Landroid/content/pm/PackageManager;", &[]) {
            Ok(val) => val,
            Err(_) => return None,
        };
        let pm = match pm_val.l() {
            Ok(pm) if !pm.as_raw().is_null() => pm,
            _ => return None,
        };

        let pkg_jstring = match env.new_string(package_name) {
            Ok(s) => s,
            Err(_) => return None,
        };

        let pkg_info_val = match env.call_method(&pm, "getPackageInfo", "(Ljava/lang/String;I)Landroid/content/pm/PackageInfo;", &[JValue::Object(&pkg_jstring), JValue::Int(0)]) {
            Ok(val) => val,
            Err(_) => {
                if env.exception_check().unwrap_or(false) {
                    let _ = env.exception_clear();
                }
                return None;
            }
        };

        let pkg_info = match pkg_info_val.l() {
            Ok(p) if !p.as_raw().is_null() => p,
            _ => return None,
        };

        let vname_field = match env.get_field(&pkg_info, "versionName", "Ljava/lang/String;") {
            Ok(f) => f,
            Err(_) => return None,
        };

        let vname_obj = match vname_field.l() {
            Ok(s) if !s.as_raw().is_null() => s,
            _ => return None,
        };

        let vname_jstr = jni::objects::JString::from(vname_obj);
        match env.get_string(&vname_jstr) {
            Ok(java_str) => Some(java_str.to_string_lossy().into_owned()),
            Err(_) => None,
        }
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

        let _ = env.call_method(&activity, "startActivity", "(Landroid/content/Intent;)V", &[JValue::Object(&intent)]);
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
        }
    }
}

fn uninstall_package(app: &AndroidApp, package_name: &str) {
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

        let uri_str = format!("package:{}", package_name);
        let uri_jstring = match env.new_string(&uri_str) {
            Ok(s) => s,
            Err(_) => return,
        };
        let uri_val = match env.call_static_method("android/net/Uri", "parse", "(Ljava/lang/String;)Landroid/net/Uri;", &[JValue::Object(&uri_jstring)]) {
            Ok(v) => v,
            Err(_) => return,
        };
        let uri_obj = match uri_val.l() {
            Ok(u) if !u.as_raw().is_null() => u,
            _ => return,
        };

        let action_delete = match env.new_string("android.intent.action.DELETE") {
            Ok(s) => s,
            Err(_) => return,
        };
        let intent_obj = match env.new_object("android/content/Intent", "(Ljava/lang/String;Landroid/net/Uri;)V", &[JValue::Object(&action_delete), JValue::Object(&uri_obj)]) {
            Ok(i) => i,
            Err(_) => return,
        };

        let _ = env.call_method(&activity, "startActivity", "(Landroid/content/Intent;)V", &[JValue::Object(&intent_obj)]);
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
        }
    }
}

fn install_module(app: &AndroidApp, app_id: &str) {
    let vm_ptr = app.vm_as_ptr();
    let activity_ptr = app.activity_as_ptr();
    if vm_ptr.is_null() || activity_ptr.is_null() {
        return;
    }

    // Check if direct APK exists locally in ArtCraft Download workspace
    let local_apk_name = match app_id {
        "craft-launcher" => "CraftLauncher.apk",
        "photocraft" => "PhotoCraft.apk",
        "cadcraft" => "CADCraft.apk",
        "vectorcraft" => "VectorCraft.apk",
        _ => "",
    };
    let local_apk_path = format!("/sdcard/Download/ArtCraft/{}", local_apk_name);
    let has_local_apk = !local_apk_name.is_empty() && std::path::Path::new(&local_apk_path).exists();

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

        if has_local_apk {
            log::info!("Found local APK at {}, invoking Android PackageInstaller...", local_apk_path);
            let file_uri_str = format!("file://{}", local_apk_path);
            let uri_jstring = match env.new_string(&file_uri_str) {
                Ok(s) => s,
                Err(_) => return,
            };
            let uri_val = match env.call_static_method("android/net/Uri", "parse", "(Ljava/lang/String;)Landroid/net/Uri;", &[JValue::Object(&uri_jstring)]) {
                Ok(v) => v,
                Err(_) => return,
            };
            let uri_obj = match uri_val.l() {
                Ok(u) if !u.as_raw().is_null() => u,
                _ => return,
            };

            let action_view = match env.new_string("android.intent.action.VIEW") {
                Ok(s) => s,
                Err(_) => return,
            };
            let intent_obj = match env.new_object("android/content/Intent", "(Ljava/lang/String;)V", &[JValue::Object(&action_view)]) {
                Ok(i) => i,
                Err(_) => return,
            };

            let mime_jstring = match env.new_string("application/vnd.android.package-archive") {
                Ok(s) => s,
                Err(_) => return,
            };
            let _ = env.call_method(&intent_obj, "setDataAndType", "(Landroid/net/Uri;Ljava/lang/String;)Landroid/content/Intent;", &[JValue::Object(&uri_obj), JValue::Object(&mime_jstring)]);
            
            // FLAG_ACTIVITY_NEW_TASK (0x10000000) | FLAG_GRANT_READ_URI_PERMISSION (0x00000001)
            let flags: i32 = 0x10000000 | 0x00000001;
            let _ = env.call_method(&intent_obj, "setFlags", "(I)Landroid/content/Intent;", &[JValue::Int(flags)]);

            let _ = env.call_method(&activity, "startActivity", "(Landroid/content/Intent;)V", &[JValue::Object(&intent_obj)]);
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_clear();
            }
            return;
        }

        // Open browser to GitHub Releases download page for manual/direct APK installation
        let url_str = format!("https://github.com/Lennowlen/{}/releases", app_id);
        let url_jstring = match env.new_string(&url_str) {
            Ok(s) => s,
            Err(_) => return,
        };
        let uri_val = match env.call_static_method("android/net/Uri", "parse", "(Ljava/lang/String;)Landroid/net/Uri;", &[JValue::Object(&url_jstring)]) {
            Ok(v) => v,
            Err(_) => return,
        };
        let uri_obj = match uri_val.l() {
            Ok(u) if !u.as_raw().is_null() => u,
            _ => return,
        };

        let action_view = match env.new_string("android.intent.action.VIEW") {
            Ok(s) => s,
            Err(_) => return,
        };
        let intent_obj = match env.new_object("android/content/Intent", "(Ljava/lang/String;Landroid/net/Uri;)V", &[JValue::Object(&action_view), JValue::Object(&uri_obj)]) {
            Ok(i) => i,
            Err(_) => return,
        };

        let _ = env.call_method(&activity, "startActivity", "(Landroid/content/Intent;)V", &[JValue::Object(&intent_obj)]);
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
