package com.sanichka.yapper

import android.Manifest
import android.app.Activity
import android.content.pm.PackageManager
import android.os.Build
import androidx.core.content.ContextCompat
import app.tauri.annotation.Command
import app.tauri.annotation.Permission
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

/**
 * Tauri Mobile Plugin exposing Android background execution and notification permissions.
 */
@TauriPlugin(
    permissions = [
        Permission(
            strings = [Manifest.permission.POST_NOTIFICATIONS],
            alias = "postNotification"
        )
    ]
)
class TtsServicePlugin(private val activity: Activity) : Plugin(activity) {

    @Command
    fun startForegroundService(invoke: Invoke) {
        RustBridge.startService()
        val ret = JSObject().apply {
            put("started", true)
        }
        invoke.resolve(ret)
    }

    @Command
    fun stopForegroundService(invoke: Invoke) {
        RustBridge.stopService()
        val ret = JSObject().apply {
            put("stopped", true)
        }
        invoke.resolve(ret)
    }

    @Command
    fun checkNotificationPermission(invoke: Invoke) {
        val granted = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            ContextCompat.checkSelfPermission(
                activity,
                Manifest.permission.POST_NOTIFICATIONS
            ) == PackageManager.PERMISSION_GRANTED
        } else {
            true
        }
        val ret = JSObject().apply {
            put("granted", granted)
        }
        invoke.resolve(ret)
    }

    @Command
    fun requestNotificationPermission(invoke: Invoke) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            val granted = ContextCompat.checkSelfPermission(
                activity,
                Manifest.permission.POST_NOTIFICATIONS
            ) == PackageManager.PERMISSION_GRANTED
            if (granted) {
                val ret = JSObject().apply { put("granted", true) }
                invoke.resolve(ret)
            } else {
                requestPermissionForAlias("postNotification", invoke, "onPermissionResult")
            }
        } else {
            val ret = JSObject().apply { put("granted", true) }
            invoke.resolve(ret)
        }
    }

    @app.tauri.annotation.PermissionCallback
    private fun onPermissionResult(invoke: Invoke) {
        val granted = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            ContextCompat.checkSelfPermission(
                activity,
                Manifest.permission.POST_NOTIFICATIONS
            ) == PackageManager.PERMISSION_GRANTED
        } else {
            true
        }
        val ret = JSObject().apply {
            put("granted", granted)
        }
        invoke.resolve(ret)
    }
}
