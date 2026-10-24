package com.sanichka.yapper

import android.content.Context
import android.content.Intent
import android.os.Build
import android.util.Log
import androidx.annotation.Keep
import java.lang.ref.WeakReference

/**
 * JNI bridge between the Kotlin Android Foreground Service and the Rust Core.
 */
@Keep
object RustBridge {
    private const val TAG = "RustBridge"

    private var appContext: Context? = null
    private var serviceRef: WeakReference<TtsForegroundService>? = null

    // Load the compiled Rust cdylib
    init {
        try {
            System.loadLibrary("tiktok_tts_app_lib")
            Log.i(TAG, "Loaded tiktok_tts_app_lib successfully")
        } catch (e: UnsatisfiedLinkError) {
            Log.e(TAG, "Could not load tiktok_tts_app_lib: ${e.message}")
        }
    }

    // Native JNI functions implemented in android_bridge.rs
    @Keep @JvmStatic external fun nativeHandleAction(action: String)
    @Keep @JvmStatic external fun nativeOnAudioCompleted(seq: Long)
    @Keep @JvmStatic external fun nativeInitJvm()
    @Keep @JvmStatic external fun nativeConnect(username: String)
    @Keep @JvmStatic external fun nativeSpeak(text: String)
    @Keep @JvmStatic external fun nativeDisconnect()

    @Keep
    @JvmStatic
    fun init(context: Context) {
        if (appContext == null) {
            appContext = context.applicationContext
        }
        try {
            nativeInitJvm()
        } catch (e: Throwable) {
            Log.w(TAG, "nativeInitJvm not ready yet: ${e.message}")
        }
    }

    @Keep
    @JvmStatic
    fun registerService(service: TtsForegroundService) {
        serviceRef = WeakReference(service)
    }

    @Keep
    @JvmStatic
    fun unregisterService() {
        serviceRef = null
    }

    @Keep
    @JvmStatic
    fun startService() {
        try {
            val context = appContext ?: return
            val intent = Intent(context, TtsForegroundService::class.java).apply {
                action = TtsForegroundService.ACTION_INIT
            }
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                context.startForegroundService(intent)
            } else {
                context.startService(intent)
            }
        } catch (e: Throwable) {
            Log.e(TAG, "Failed to start TtsForegroundService: ${e.message}", e)
        }
    }

    @Keep
    @JvmStatic
    fun stopService() {
        try {
            val context = appContext ?: return
            val intent = Intent(context, TtsForegroundService::class.java).apply {
                action = TtsForegroundService.ACTION_STOP_SERVICE
            }
            context.stopService(intent)
        } catch (e: Throwable) {
            Log.e(TAG, "Failed to stop TtsForegroundService: ${e.message}", e)
        }
    }

    @Keep
    @JvmStatic
    fun updateNotification(
        title: String,
        text: String,
        queueDepth: Int,
        isPaused: Boolean,
        isConnected: Boolean
    ) {
        try {
            serviceRef?.get()?.updateNotificationState(
                title = title,
                subtitle = text,
                queueDepth = queueDepth,
                isPaused = isPaused,
                isConnected = isConnected
            )
        } catch (e: Throwable) {
            Log.e(TAG, "Failed to update notification: ${e.message}", e)
        }
    }

    @Keep
    @JvmStatic
    fun playAudio(pcmBytes: ByteArray, sampleRate: Int, seq: Long) {
        try {
            var service = serviceRef?.get()
            if (service == null) {
                Log.w("YapperRust", "playAudio called but TtsForegroundService is not active; starting service")
                startService()
                for (i in 0..6) {
                    Thread.sleep(50)
                    service = serviceRef?.get()
                    if (service != null) break
                }
            }
            if (service != null) {
                Log.i("YapperRust", "RustBridge delegating playAudio to service for seq=$seq, bytes=${pcmBytes.size}")
                service.playAudio(pcmBytes, sampleRate, seq)
            } else {
                Log.e("YapperRust", "TtsForegroundService still not active after wait, acknowledging audio seq $seq")
                onAudioCompleted(seq)
            }
        } catch (e: Throwable) {
            Log.e("YapperRust", "RustBridge failed to play audio: ${e.message}", e)
            onAudioCompleted(seq)
        }
    }

    @Keep
    @JvmStatic
    fun stopAudio() {
        try {
            serviceRef?.get()?.stopAudio()
        } catch (e: Throwable) {
            Log.e(TAG, "Failed to stop audio: ${e.message}", e)
        }
    }

    @Keep
    @JvmStatic
    fun handleNotificationAction(action: String) {
        try {
            nativeHandleAction(action)
        } catch (e: Throwable) {
            Log.e(TAG, "Failed to send notification action '$action' to Rust: ${e.message}", e)
        }
    }

    @Keep
    @JvmStatic
    fun onAudioCompleted(seq: Long) {
        try {
            nativeOnAudioCompleted(seq)
        } catch (e: Throwable) {
            Log.e(TAG, "Failed to notify audio completion for seq $seq: ${e.message}", e)
        }
    }

    @Keep
    @JvmStatic
    fun connect(username: String) {
        try {
            startService()
            nativeConnect(username)
        } catch (e: Throwable) {
            Log.e(TAG, "Failed to call nativeConnect: ${e.message}", e)
        }
    }

    @Keep
    @JvmStatic
    fun speak(text: String) {
        try {
            nativeSpeak(text)
        } catch (e: Throwable) {
            Log.e(TAG, "Failed to call nativeSpeak: ${e.message}", e)
        }
    }

    @Keep
    @JvmStatic
    fun disconnect() {
        try {
            nativeDisconnect()
        } catch (e: Throwable) {
            Log.e(TAG, "Failed to call nativeDisconnect: ${e.message}", e)
        }
    }
}
