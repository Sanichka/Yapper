package com.sanichka.yapper

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log
import androidx.annotation.Keep

/**
 * BroadcastReceiver triggered by user taps on Notification shade media action buttons.
 */
@Keep
class NotificationActionReceiver : BroadcastReceiver() {
    companion object {
        private const val TAG = "NotificationActionRx"
    }

    override fun onReceive(context: Context, intent: Intent) {
        RustBridge.init(context)
        val action = intent.action ?: return
        Log.i(TAG, "Notification action received: $action")

        try {
            when (action) {
                TtsForegroundService.ACTION_CONNECT -> {
                    val username = intent.getStringExtra("username")?.trim() ?: "kimonosha"
                    Log.i(TAG, "Connecting to TikTok user: $username")
                    RustBridge.connect(username)
                }
                TtsForegroundService.ACTION_SPEAK -> {
                    val text = intent.getStringExtra("text") ?: "Тест синтезу мовлення"
                    Log.i(TAG, "Speaking test text: $text")
                    RustBridge.speak(text)
                }
                TtsForegroundService.ACTION_DISCONNECT -> {
                    Log.i(TAG, "Disconnecting from TikTok LIVE")
                    RustBridge.disconnect()
                }
                TtsForegroundService.ACTION_STOP_PAUSE,
                TtsForegroundService.ACTION_SKIP -> {
                    RustBridge.stopAudio()
                    RustBridge.handleNotificationAction(action)
                }
                else -> {
                    RustBridge.handleNotificationAction(action)
                }
            }
        } catch (e: Throwable) {
            Log.e(TAG, "Error handling notification action: ${e.message}", e)
        }
    }
}
