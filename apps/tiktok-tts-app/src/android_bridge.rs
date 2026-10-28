//! JNI Bridge and Android Foreground Service communication.
//!
//! Provides bidirectional state synchronization between the Rust Core,
//! the Android TtsForegroundService, and the notification shade controller.

use std::sync::Arc;

#[cfg(target_os = "android")]
use std::sync::OnceLock;

#[cfg(target_os = "android")]
static CORE_INSTANCE: OnceLock<Arc<crate::core::Core>> = OnceLock::new();
#[cfg(target_os = "android")]
static JVM_INSTANCE: OnceLock<jni::JavaVM> = OnceLock::new();
#[cfg(target_os = "android")]
static RUST_BRIDGE_CLASS: OnceLock<jni::objects::GlobalRef> = OnceLock::new();

/// Registers the application Core instance for incoming JNI callbacks from the Android Service.
pub fn register_core(core: Arc<crate::core::Core>) {
    #[cfg(target_os = "android")]
    {
        let _ = CORE_INSTANCE.set(core);
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = core;
    }
}

/// Returns whether the current runtime target is Android.
pub fn is_android() -> bool {
    cfg!(target_os = "android")
}

#[cfg(target_os = "android")]
fn with_bridge_env<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut jni::JNIEnv, &jni::objects::GlobalRef) -> Result<R, jni::errors::Error>,
{
    let vm = match JVM_INSTANCE.get() {
        Some(v) => v,
        None => {
            android_log(6, "with_bridge_env: JVM_INSTANCE is None");
            return None;
        }
    };
    let class = match RUST_BRIDGE_CLASS.get() {
        Some(c) => c,
        None => {
            android_log(6, "with_bridge_env: RUST_BRIDGE_CLASS is None");
            return None;
        }
    };
    let mut env = match vm.attach_current_thread() {
        Ok(e) => e,
        Err(err) => {
            android_log(6, &format!("with_bridge_env: attach_current_thread failed: {err}"));
            return None;
        }
    };

    let res = f(&mut env, class);

    if env.exception_check().unwrap_or(false) {
        android_log(6, "with_bridge_env: Java exception occurred during JNI call");
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }

    match res {
        Ok(val) => Some(val),
        Err(err) => {
            android_log(6, &format!("with_bridge_env: JNI call failed: {err}"));
            None
        }
    }
}

#[cfg(target_os = "android")]
#[link(name = "log")]
extern "C" {
    fn __android_log_print(
        prio: std::os::raw::c_int,
        tag: *const std::os::raw::c_char,
        fmt: *const std::os::raw::c_char,
        ...
    ) -> std::os::raw::c_int;
}

#[cfg(target_os = "android")]
pub fn android_log(level: i32, msg: &str) {
    use std::ffi::CString;
    let tag = CString::new("YapperRust").unwrap();
    let fmt = CString::new("%s").unwrap();
    if let Ok(cval) = CString::new(msg) {
        unsafe {
            __android_log_print(level, tag.as_ptr(), fmt.as_ptr(), cval.as_ptr());
        }
    }
}

/// Notifies the Android Foreground Service to update the notification shade title,
/// currently speaking comment, queue counter, and play/pause action button.
pub fn update_notification(
    title: &str,
    text: &str,
    queue_depth: usize,
    is_paused: bool,
    is_connected: bool,
) {
    #[cfg(target_os = "android")]
    {
        android_log(
            4,
            &format!("update_notification: title='{title}', q={queue_depth}, paused={is_paused}, connected={is_connected}"),
        );
        with_bridge_env(|env, class| {
            let jtitle = env.new_string(title)?;
            let jtext = env.new_string(text)?;
            env.call_static_method(
                class,
                "updateNotification",
                "(Ljava/lang/String;Ljava/lang/String;IZZ)V",
                &[
                    (&jtitle).into(),
                    (&jtext).into(),
                    (queue_depth as i32).into(),
                    is_paused.into(),
                    is_connected.into(),
                ],
            )?;
            Ok(())
        });
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (title, text, queue_depth, is_paused, is_connected);
    }
}

/// Commands the Android Foreground Service to start if not already active.
pub fn start_service() {
    #[cfg(target_os = "android")]
    {
        android_log(4, "Rust commands start_service()");
        with_bridge_env(|env, class| {
            env.call_static_method(class, "startService", "()V", &[])?;
            Ok(())
        });
    }
}

/// Commands the Android Foreground Service to stop and release locks.
pub fn stop_service() {
    #[cfg(target_os = "android")]
    {
        android_log(4, "Rust commands stop_service()");
        with_bridge_env(|env, class| {
            env.call_static_method(class, "stopService", "()V", &[])?;
            Ok(())
        });
    }
}

#[cfg(target_os = "android")]
fn call_kotlin_play_audio(pcm_bytes: &[u8], sample_rate: u32, seq: u64) -> Result<(), String> {
    android_log(
        4,
        &format!("call_kotlin_play_audio: seq={seq}, bytes={}, sr={sample_rate}", pcm_bytes.len()),
    );
    with_bridge_env(|env, class| {
        let byte_array = env.byte_array_from_slice(pcm_bytes)?;
        env.call_static_method(
            class,
            "playAudio",
            "([BIJ)V",
            &[
                (&byte_array).into(),
                (sample_rate as i32).into(),
                (seq as i64).into(),
            ],
        )?;
        Ok(())
    })
    .ok_or_else(|| "Failed to delegate playAudio to Kotlin Foreground Service".to_string())
}

#[cfg(target_os = "android")]
fn call_kotlin_stop_audio() {
    android_log(4, "call_kotlin_stop_audio()");
    with_bridge_env(|env, class| {
        env.call_static_method(class, "stopAudio", "()V", &[])?;
        Ok(())
    });
}

/// Native Android AudioSink that delegates playback directly to the Kotlin Foreground Service.
#[derive(Debug, Default)]
pub struct AndroidAudioSink;

impl crate::audio_sink::AudioSink for AndroidAudioSink {
    fn play(
        &self,
        seq: u64,
        samples: &[f32],
        sample_rate: u32,
        volume: f32,
        _cancel: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<(), String> {
        #[cfg(target_os = "android")]
        {
            let pcm = crate::audio_sink::samples_to_pcm16_le_with_volume(samples, volume);
            call_kotlin_play_audio(&pcm, sample_rate, seq)
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = (seq, samples, sample_rate, volume, _cancel);
            Ok(())
        }
    }

    fn stop(&self) {
        #[cfg(target_os = "android")]
        {
            call_kotlin_stop_audio();
        }
    }
}

// ---------------------------------------------------------------------------
// JNI Entry Points (Called from com.sanichka.yapper.RustBridge in Kotlin)
// ---------------------------------------------------------------------------

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn JNI_OnLoad(
    vm: jni::JavaVM,
    _reserved: *mut std::ffi::c_void,
) -> jni::sys::jint {
    android_log(4, "JNI_OnLoad called by Android VM");
    if let Ok(mut env) = vm.get_env() {
        if let Ok(class) = env.find_class("com/sanichka/yapper/RustBridge") {
            if let Ok(global) = env.new_global_ref(class) {
                let _ = RUST_BRIDGE_CLASS.set(global);
                android_log(4, "JNI_OnLoad: RUST_BRIDGE_CLASS cached successfully");
            }
        }
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
        }
    }
    let _ = JVM_INSTANCE.set(vm);
    jni::sys::JNI_VERSION_1_6
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_com_sanichka_yapper_RustBridge_nativeHandleAction(
    mut env: jni::JNIEnv,
    _class: jni::objects::JClass,
    action: jni::objects::JString,
) {
    let action_str: String = match env.get_string(&action) {
        Ok(s) => s.into(),
        Err(e) => {
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_clear();
            }
            android_log(6, &format!("Failed to get action string from JNI: {e}"));
            return;
        }
    };
    android_log(4, &format!("nativeHandleAction received: {action_str}"));

    if let Some(core) = CORE_INSTANCE.get() {
        core.handle_notification_action(&action_str);
    } else {
        android_log(5, "RustBridge received action before Core initialized");
    }
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_com_sanichka_yapper_RustBridge_nativeOnAudioCompleted(
    _env: jni::JNIEnv,
    _class: jni::objects::JClass,
    seq: jni::sys::jlong,
) {
    if let Some(core) = CORE_INSTANCE.get() {
        core.ack(seq as u64);
    }
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_com_sanichka_yapper_RustBridge_nativeInitJvm(
    env: jni::JNIEnv,
    class: jni::objects::JClass,
) {
    android_log(4, "nativeInitJvm called from Kotlin");
    if let Ok(vm) = env.get_java_vm() {
        let _ = JVM_INSTANCE.set(vm);
    }
    if let Ok(global) = env.new_global_ref(class) {
        let _ = RUST_BRIDGE_CLASS.set(global);
        android_log(4, "nativeInitJvm: RUST_BRIDGE_CLASS cached successfully");
    }
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_com_sanichka_yapper_RustBridge_nativeConnect(
    mut env: jni::JNIEnv,
    _class: jni::objects::JClass,
    username: jni::objects::JString,
) {
    let username_str: String = match env.get_string(&username) {
        Ok(s) => s.into(),
        Err(e) => {
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_clear();
            }
            android_log(6, &format!("Failed to get username string from JNI: {e}"));
            return;
        }
    };
    android_log(4, &format!("nativeConnect received for: {username_str}"));
    if let Some(core) = CORE_INSTANCE.get() {
        let core = core.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = core.connect(&username_str).await {
                android_log(6, &format!("Failed to connect to @{username_str}: {e}"));
                core.log("error", format!("Failed to connect to @{username_str}: {e}"));
            }
        });
    } else {
        android_log(5, "RustBridge nativeConnect called before Core initialized");
    }
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_com_sanichka_yapper_RustBridge_nativeSpeak(
    mut env: jni::JNIEnv,
    _class: jni::objects::JClass,
    text: jni::objects::JString,
) {
    let text_str: String = match env.get_string(&text) {
        Ok(s) => s.into(),
        Err(e) => {
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_clear();
            }
            android_log(6, &format!("Failed to get speak text string from JNI: {e}"));
            return;
        }
    };
    android_log(4, &format!("nativeSpeak received: {text_str}"));
    if let Some(core) = CORE_INSTANCE.get() {
        if let Err(e) = core.speak(&text_str) {
            android_log(6, &format!("Failed to speak: {e}"));
            core.log("error", format!("Failed to speak: {e}"));
        }
    } else {
        android_log(5, "RustBridge nativeSpeak called before Core initialized");
    }
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_com_sanichka_yapper_RustBridge_nativeDisconnect(
    _env: jni::JNIEnv,
    _class: jni::objects::JClass,
) {
    android_log(4, "nativeDisconnect received");
    if let Some(core) = CORE_INSTANCE.get() {
        core.disconnect();
    }
}
