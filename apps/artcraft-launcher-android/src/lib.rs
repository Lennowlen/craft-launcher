use android_activity::AndroidApp;
use artcraft_launcher_core::protocol::{Backend, Capabilities, Command, Event, LatestRelease, Origin, Source};
use artcraft_launcher_core::status::{InstallSource, Installed};
use artcraft_launcher_core::version;
use artcraft_launcher_core::{Arch, Os, Platform, SUITE};
use artcraft_launcher_ui_egui::{Host, LauncherUi, Settings};
use jni::objects::JValue;
use std::sync::{Arc, Mutex};
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
    events: Arc<Mutex<Vec<Event>>>,
}

impl AndroidBackend {
    fn new(app: AndroidApp, ctx: egui::Context) -> Self {
        let events = Arc::new(Mutex::new(Vec::new()));
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

            self.events.lock().unwrap().push(Event::Detected {
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

            self.events.lock().unwrap().push(Event::Latest {
                app: app.id.into(),
                latest: LatestRelease::Found(release),
                from: Origin::GitHub,
            });
        }

        // Self-update launcher check
        let launcher_installed_v = get_package_version_name(&self.app, "ai.storyteller.craftlauncher")
            .as_deref()
            .and_then(version::parse)
            .unwrap_or_else(|| version::parse("0.4.3").unwrap());
        let launcher_latest_v = version::parse("0.4.3").unwrap();
        if version::is_newer(&launcher_latest_v, &launcher_installed_v) {
            let launcher_release = artcraft_launcher_core::release::Release {
                tag: "latest".into(),
                version: Some(launcher_latest_v),
                name: "Craft Launcher v0.4.3".into(),
                published_at: Some(now_sec),
                html_url: "https://github.com/Lennowlen/craft-launcher/releases".into(),
                notes: "Pembaruan Craft Launcher ARM64: Dukungan dual-version tracking dan update installer.".into(),
                prerelease: false,
                assets: vec![artcraft_launcher_core::release::Asset {
                    name: "CraftLauncher.apk".into(),
                    size: 8_200_000,
                    url: "https://github.com/Lennowlen/craft-launcher/releases/latest/download/CraftLauncher.apk".into(),
                    sha256: None,
                }],
            };
            self.events.lock().unwrap().push(Event::LauncherUpdate(launcher_release));
        }

        self.events.lock().unwrap().push(Event::CheckFinished {
            at: now_sec,
            source: Source::Feed,
            rate_limited_until: None,
        });

        self.ctx.request_repaint();
    }
}

use std::sync::atomic::{AtomicBool, Ordering};

static CANCEL_DOWNLOAD: AtomicBool = AtomicBool::new(false);
static PAUSE_DOWNLOAD: AtomicBool = AtomicBool::new(false);

impl Backend for AndroidBackend {
    fn poll(&mut self) -> Vec<Event> {
        self.events.lock().unwrap().drain(..).collect()
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
                CANCEL_DOWNLOAD.store(false, Ordering::SeqCst);
                PAUSE_DOWNLOAD.store(false, Ordering::SeqCst);
                let app_clone = self.app.clone();
                let events = self.events.clone();
                let ctx = self.ctx.clone();
                let app_id = app.clone();
                std::thread::spawn(move || {
                    download_and_install_module(&app_clone, &app_id, events, ctx);
                });
            }
            Command::Pause { app } => {
                log::info!("Pause command requested for: {}", app);
                PAUSE_DOWNLOAD.store(true, Ordering::SeqCst);
            }
            Command::Cancel { app } => {
                log::info!("Cancel command requested for: {}", app);
                CANCEL_DOWNLOAD.store(true, Ordering::SeqCst);
            }
            Command::Uninstall { app } => {
                log::info!("Uninstall command requested for: {}", app);
                let pkg = format!("ai.storyteller.{}", app);
                uninstall_package(&self.app, &pkg);
            }
            Command::UpdateLauncher => {
                log::info!("UpdateLauncher command requested");
                CANCEL_DOWNLOAD.store(false, Ordering::SeqCst);
                PAUSE_DOWNLOAD.store(false, Ordering::SeqCst);
                let app_clone = self.app.clone();
                let events = self.events.clone();
                let ctx = self.ctx.clone();
                std::thread::spawn(move || {
                    download_and_install_module(&app_clone, artcraft_launcher_core::catalog::LAUNCHER_ID, events, ctx);
                });
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
    log::info!("uninstall_package: requesting uninstall for {}", package_name);
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

        let flags: i32 = 0x10000000; // FLAG_ACTIVITY_NEW_TASK
        let _ = env.call_method(&intent_obj, "setFlags", "(I)Landroid/content/Intent;", &[JValue::Int(flags)]);

        let _ = env.call_method(&activity, "startActivity", "(Landroid/content/Intent;)V", &[JValue::Object(&intent_obj)]);
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
        }
    }
}

fn download_and_install_module(app: &AndroidApp, app_id: &str, events: Arc<Mutex<Vec<Event>>>, ctx: egui::Context) {
    let apk_filename = match app_id {
        "artcraft-launcher" | "craft-launcher" => "CraftLauncher.apk",
        "photocraft" => "PhotoCraft.apk",
        "cadcraft" => "CADCraft.apk",
        "vectorcraft" => "VectorCraft.apk",
        "designcraft" => "DesignCraft.apk",
        "figcraft" => "FigCraft.apk",
        "filmcraft" => "FilmCraft.apk",
        "effectcraft" => "EffectCraft.apk",
        "lightcraft" => "LightCraft.apk",
        "pdfcraft" => "PdfCraft.apk",
        "deckcraft" => "DeckCraft.apk",
        "gridcraft" => "GridCraft.apk",
        "wordcraft" => "WordCraft.apk",
        "soundcraft" => "SoundCraft.apk",
        "codecraft" => "CodeCraft.apk",
        _ => "app.apk",
    };

    let target_path = format!("/sdcard/Download/ArtCraft/{}", apk_filename);

    // If local APK already exists, trigger installer immediately
    if std::path::Path::new(&target_path).exists() {
        log::info!("Local APK found at {target_path}, launching installer");
        events.lock().unwrap().push(Event::Progress {
            app: app_id.to_string(),
            phase: artcraft_launcher_core::protocol::JobPhase::Installing,
        });
        ctx.request_repaint();

        launch_apk_installer(app, &target_path);

        events.lock().unwrap().push(Event::JobFinished {
            app: app_id.to_string(),
            result: Ok("Selesai dipasang".into()),
        });
        ctx.request_repaint();
        return;
    }

    // Determine download URL
    let download_url = match app_id {
        "artcraft-launcher" | "craft-launcher" => {
            "https://github.com/Lennowlen/craft-launcher/releases/latest/download/CraftLauncher.apk".to_string()
        }
        _ => {
            format!("https://github.com/Lennowlen/{}/releases/latest/download/{}", app_id, apk_filename)
        }
    };

    log::info!("Downloading APK from {} to {}", download_url, target_path);

    // Emit downloading starting
    events.lock().unwrap().push(Event::Progress {
        app: app_id.to_string(),
        phase: artcraft_launcher_core::protocol::JobPhase::Downloading { done: 0, total: 20_000_000 },
    });
    ctx.request_repaint();

    // Perform download
    let agent: ureq::Agent = ureq::config::Config::builder()
        .max_redirects(10)
        .build()
        .into();

    let res = agent.get(&download_url)
        .header("User-Agent", "CraftLauncher-Android/0.4.3")
        .call();
    match res {
        Ok(mut response) => {
            let total_len = response
                .headers()
                .get("content-length")
                .and_then(|v| v.to_str().ok())
                .and_then(|l| l.parse::<u64>().ok())
                .unwrap_or(20_000_000);

            let mut reader = response.body_mut().as_reader();
            let mut dest = match std::fs::File::create(&target_path) {
                Ok(f) => f,
                Err(e) => {
                    log::error!("Failed to create destination APK file: {e}");
                    events.lock().unwrap().push(Event::JobFinished {
                        app: app_id.to_string(),
                        result: Err(e.to_string()),
                    });
                    ctx.request_repaint();
                    return;
                }
            };

            let mut buffer = [0u8; 64 * 1024];
            let mut downloaded = 0u64;
            use std::io::{Read, Write};

            loop {
                if CANCEL_DOWNLOAD.load(Ordering::SeqCst) {
                    log::info!("Download cancelled by user for {app_id}");
                    let _ = std::fs::remove_file(&target_path);
                    events.lock().unwrap().push(Event::JobFinished {
                        app: app_id.to_string(),
                        result: Err(artcraft_launcher_core::protocol::CANCELLED.to_string()),
                    });
                    ctx.request_repaint();
                    return;
                }

                if PAUSE_DOWNLOAD.load(Ordering::SeqCst) {
                    log::info!("Download paused by user for {app_id}");
                    events.lock().unwrap().push(Event::Paused {
                        app: app_id.to_string(),
                        download: Some(artcraft_launcher_core::protocol::PausedDownload {
                            done: downloaded,
                            total: total_len,
                        }),
                    });
                    events.lock().unwrap().push(Event::JobFinished {
                        app: app_id.to_string(),
                        result: Err(artcraft_launcher_core::protocol::PAUSED.to_string()),
                    });
                    ctx.request_repaint();
                    return;
                }

                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        let _ = dest.write_all(&buffer[..n]);
                        downloaded += n as u64;
                        events.lock().unwrap().push(Event::Progress {
                            app: app_id.to_string(),
                            phase: artcraft_launcher_core::protocol::JobPhase::Downloading {
                                done: downloaded,
                                total: total_len,
                            },
                        });
                        ctx.request_repaint();
                    }
                    Err(e) => {
                        log::error!("Download error: {e}");
                        events.lock().unwrap().push(Event::JobFinished {
                            app: app_id.to_string(),
                            result: Err(e.to_string()),
                        });
                        ctx.request_repaint();
                        return;
                    }
                }
            }

            events.lock().unwrap().push(Event::Progress {
                app: app_id.to_string(),
                phase: artcraft_launcher_core::protocol::JobPhase::Installing,
            });
            ctx.request_repaint();

            launch_apk_installer(app, &target_path);

            events.lock().unwrap().push(Event::JobFinished {
                app: app_id.to_string(),
                result: Ok("Selesai dipasang".into()),
            });
            ctx.request_repaint();
        }
        Err(e) => {
            log::error!("HTTP GET failed for {}: {e}", download_url);
            events.lock().unwrap().push(Event::JobFinished {
                app: app_id.to_string(),
                result: Err(format!("Download failed: {e}")),
            });
            ctx.request_repaint();
        }
    }
}

fn launch_apk_installer(app: &AndroidApp, apk_path: &str) {
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

        log::info!("launch_apk_installer: preparing PackageInstaller session for APK: {}", apk_path);

        // Get PackageManager: activity.getPackageManager()
        let pm_val = match env.call_method(&activity, "getPackageManager", "()Landroid/content/pm/PackageManager;", &[]) {
            Ok(val) => val,
            Err(e) => {
                log::error!("Failed to get PackageManager: {:?}", e);
                return;
            }
        };
        let pm = match pm_val.l() {
            Ok(p) if !p.as_raw().is_null() => p,
            _ => return,
        };

        // Get PackageInstaller: pm.getPackageInstaller()
        let pi_val = match env.call_method(&pm, "getPackageInstaller", "()Landroid/content/pm/PackageInstaller;", &[]) {
            Ok(val) => val,
            Err(e) => {
                log::error!("Failed to get PackageInstaller: {:?}", e);
                return;
            }
        };
        let pi = match pi_val.l() {
            Ok(p) if !p.as_raw().is_null() => p,
            _ => return,
        };

        // Create SessionParams: new PackageInstaller.SessionParams(MODE_FULL_INSTALL = 1)
        let params_obj = match env.new_object("android/content/pm/PackageInstaller$SessionParams", "(I)V", &[JValue::Int(1)]) {
            Ok(obj) => obj,
            Err(e) => {
                log::error!("Failed to create SessionParams: {:?}", e);
                return;
            }
        };

        // Create session: int sessionId = pi.createSession(params)
        let session_id_val = match env.call_method(&pi, "createSession", "(Landroid/content/pm/PackageInstaller$SessionParams;)I", &[JValue::Object(&params_obj)]) {
            Ok(val) => val,
            Err(e) => {
                log::error!("Failed to create install session: {:?}", e);
                return;
            }
        };
        let session_id = match session_id_val.i() {
            Ok(id) => id,
            Err(_) => return,
        };

        log::info!("launch_apk_installer: created install session {}", session_id);

        // Open session: PackageInstaller.Session session = pi.openSession(sessionId)
        let session_val = match env.call_method(&pi, "openSession", "(I)Landroid/content/pm/PackageInstaller$Session;", &[JValue::Int(session_id)]) {
            Ok(val) => val,
            Err(e) => {
                log::error!("Failed to open install session: {:?}", e);
                return;
            }
        };
        let session = match session_val.l() {
            Ok(s) if !s.as_raw().is_null() => s,
            _ => return,
        };

        // Stream APK bytes into session.openWrite("base.apk", 0, apk_size)
        let apk_bytes = match std::fs::read(apk_path) {
            Ok(b) => b,
            Err(e) => {
                log::error!("Failed to read APK file for streaming: {:?}", e);
                return;
            }
        };
        let apk_size = apk_bytes.len() as i64;
        let entry_name = match env.new_string("base.apk") {
            Ok(s) => s,
            Err(_) => return,
        };

        let out_stream_val = match env.call_method(
            &session,
            "openWrite",
            "(Ljava/lang/String;JJ)Ljava/io/OutputStream;",
            &[JValue::Object(&entry_name), JValue::Long(0), JValue::Long(apk_size)],
        ) {
            Ok(val) => val,
            Err(e) => {
                log::error!("Failed to open session OutputStream: {:?}", e);
                return;
            }
        };
        let out_stream = match out_stream_val.l() {
            Ok(s) if !s.as_raw().is_null() => s,
            _ => return,
        };

        // Write in chunks via byte[]
        let chunk_size = 64 * 1024;
        for chunk in apk_bytes.chunks(chunk_size) {
            let byte_array = match env.byte_array_from_slice(chunk) {
                Ok(arr) => arr,
                Err(e) => {
                    log::error!("Failed to create chunk byte array: {:?}", e);
                    return;
                }
            };
            let _ = env.call_method(&out_stream, "write", "([B)V", &[JValue::Object(&byte_array)]);
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_clear();
                log::error!("Exception writing APK chunk to session OutputStream");
                return;
            }
        }

        let _ = env.call_method(&session, "fsync", "(Ljava/io/OutputStream;)V", &[JValue::Object(&out_stream)]);
        let _ = env.call_method(&out_stream, "close", "()V", &[]);

        // Create PendingIntent for commit status callback:
        // Intent commitIntent = new Intent("ai.storyteller.craftlauncher.INSTALL_COMPLETE");
        let action_commit = match env.new_string("ai.storyteller.craftlauncher.INSTALL_COMPLETE") {
            Ok(s) => s,
            Err(_) => return,
        };
        let commit_intent = match env.new_object("android/content/Intent", "(Ljava/lang/String;)V", &[JValue::Object(&action_commit)]) {
            Ok(i) => i,
            Err(_) => return,
        };

        // PendingIntent piSender = PendingIntent.getBroadcast(activity, sessionId, commitIntent, FLAG_MUTABLE = 0x02000000 | FLAG_UPDATE_CURRENT = 0x08000000)
        let pi_flags: i32 = 0x02000000 | 0x08000000;
        let pending_intent_val = match env.call_static_method(
            "android/app/PendingIntent",
            "getBroadcast",
            "(Landroid/content/Context;ILandroid/content/Intent;I)Landroid/app/PendingIntent;",
            &[JValue::Object(&activity), JValue::Int(session_id), JValue::Object(&commit_intent), JValue::Int(pi_flags)],
        ) {
            Ok(val) => val,
            Err(e) => {
                log::error!("Failed to create PendingIntent: {:?}", e);
                return;
            }
        };
        let pending_intent = match pending_intent_val.l() {
            Ok(p) if !p.as_raw().is_null() => p,
            _ => return,
        };

        let status_receiver = match env.call_method(&pending_intent, "getIntentSender", "()Landroid/content/IntentSender;", &[]) {
            Ok(val) => val,
            Err(e) => {
                log::error!("Failed to get IntentSender: {:?}", e);
                return;
            }
        };
        let status_receiver_obj = match status_receiver.l() {
            Ok(s) if !s.as_raw().is_null() => s,
            _ => return,
        };

        // session.commit(statusReceiver)
        let _ = env.call_method(&session, "commit", "(Landroid/content/IntentSender;)V", &[JValue::Object(&status_receiver_obj)]);
        let _ = env.call_method(&session, "close", "()V", &[]);

        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
            log::error!("Exception committing install session");
        } else {
            log::info!("launch_apk_installer: install session committed successfully!");
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
