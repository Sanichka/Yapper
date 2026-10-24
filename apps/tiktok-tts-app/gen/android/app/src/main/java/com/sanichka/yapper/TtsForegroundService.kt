package com.sanichka.yapper

import android.annotation.SuppressLint
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioTrack
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.PowerManager
import android.support.v4.media.MediaMetadataCompat
import android.support.v4.media.session.MediaSessionCompat
import android.support.v4.media.session.PlaybackStateCompat
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.media.app.NotificationCompat.MediaStyle
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

import androidx.annotation.Keep

/**
 * Foreground Service responsible for:
 * 1. Keeping CPU and Wi-Fi active (WakeLock and WifiLock) while streaming.
 * 2. Displaying persistent MediaStyle interactive notification with dynamic stream/queue status.
 * 3. Playing neural TTS audio buffers directly via AudioTrack with immediate mid-sentence abort on Skip/Stop.
 */
@Keep
class TtsForegroundService : Service() {

    companion object {
        const val TAG = "TtsForegroundService"
        const val CHANNEL_ID = "yapper_tts_channel_v2"
        const val NOTIFICATION_ID = 2026

        const val ACTION_INIT = "com.sanichka.yapper.ACTION_INIT"
        const val ACTION_START_RESUME = "com.sanichka.yapper.ACTION_START_RESUME"
        const val ACTION_STOP_PAUSE = "com.sanichka.yapper.ACTION_STOP_PAUSE"
        const val ACTION_SKIP = "com.sanichka.yapper.ACTION_SKIP"
        const val ACTION_STOP_SERVICE = "com.sanichka.yapper.ACTION_STOP_SERVICE"
        const val ACTION_CONNECT = "com.sanichka.yapper.ACTION_CONNECT"
        const val ACTION_SPEAK = "com.sanichka.yapper.ACTION_SPEAK"
        const val ACTION_DISCONNECT = "com.sanichka.yapper.ACTION_DISCONNECT"
    }

    private var wakeLock: PowerManager.WakeLock? = null
    private var wifiLock: WifiManager.WifiLock? = null
    private var mediaSession: MediaSessionCompat? = null

    // Audio Focus & Playback
    private var audioManager: AudioManager? = null
    private var audioFocusRequest: AudioFocusRequest? = null
    private val audioExecutor: ExecutorService = Executors.newSingleThreadExecutor()
    private val isAudioCancelled = AtomicBoolean(false)
    @Volatile private var activeAudioTrack: AudioTrack? = null

    // State for notification display
    private var currentTitle: String = "Yapper • Idle"
    private var currentSubtitle: String = "Listening to live chat"
    private var currentQueueDepth: Int = 0
    private var isCurrentlyPaused: Boolean = false
    private var isCurrentlyConnected: Boolean = false

    override fun onCreate() {
        super.onCreate()
        Log.i(TAG, "Creating TtsForegroundService")
        RustBridge.registerService(this)

        createNotificationChannel()
        acquireLocks()
        initMediaSession()

        // Start foreground immediately with current state
        try {
            val notification = buildNotification()
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                startForeground(
                    NOTIFICATION_ID,
                    notification,
                    ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK
                )
            } else {
                startForeground(NOTIFICATION_ID, notification)
            }
        } catch (e: Throwable) {
            Log.e(TAG, "Failed to startForeground: ${e.message}", e)
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val action = intent?.action
        Log.d(TAG, "onStartCommand received action: $action")
        when (action) {
            ACTION_START_RESUME -> {
                RustBridge.handleNotificationAction(ACTION_START_RESUME)
            }
            ACTION_STOP_PAUSE -> {
                stopAudio()
                RustBridge.handleNotificationAction(ACTION_STOP_PAUSE)
            }
            ACTION_SKIP -> {
                stopAudio()
                RustBridge.handleNotificationAction(ACTION_SKIP)
            }
            ACTION_STOP_SERVICE -> {
                stopAudio()
                stopForeground(STOP_FOREGROUND_REMOVE)
                stopSelf()
            }
        }
        return START_STICKY
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onDestroy() {
        Log.i(TAG, "Destroying TtsForegroundService")
        stopAudio()
        audioExecutor.shutdownNow()
        releaseLocks()

        mediaSession?.apply {
            isActive = false
            release()
        }
        mediaSession = null

        RustBridge.unregisterService()
        super.onDestroy()
    }

    // -----------------------------------------------------------------------
    // Locks: WakeLock & WifiLock
    // -----------------------------------------------------------------------

    @SuppressLint("WakelockTimeout")
    private fun acquireLocks() {
        try {
            val powerManager = getSystemService(Context.POWER_SERVICE) as? PowerManager
            if (powerManager != null && wakeLock == null) {
                wakeLock = powerManager.newWakeLock(
                    PowerManager.PARTIAL_WAKE_LOCK,
                    "Yapper::TtsWakeLock"
                ).apply {
                    setReferenceCounted(false)
                    acquire()
                }
                Log.d(TAG, "Acquired PowerManager PARTIAL_WAKE_LOCK")
            }

            val wifiManager = applicationContext.getSystemService(Context.WIFI_SERVICE) as? WifiManager
            if (wifiManager != null && wifiLock == null) {
                val mode = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                    WifiManager.WIFI_MODE_FULL_HIGH_PERF
                } else {
                    @Suppress("DEPRECATION")
                    WifiManager.WIFI_MODE_FULL
                }
                wifiLock = wifiManager.createWifiLock(mode, "Yapper::TtsWifiLock").apply {
                    setReferenceCounted(false)
                    acquire()
                }
                Log.d(TAG, "Acquired WifiManager WifiLock")
            }
        } catch (e: Exception) {
            Log.e(TAG, "Error acquiring system locks: ${e.message}")
        }
    }

    private fun releaseLocks() {
        try {
            wakeLock?.let {
                if (it.isHeld) it.release()
                Log.d(TAG, "Released WakeLock")
            }
            wakeLock = null

            wifiLock?.let {
                if (it.isHeld) it.release()
                Log.d(TAG, "Released WifiLock")
            }
            wifiLock = null
        } catch (e: Exception) {
            Log.e(TAG, "Error releasing system locks: ${e.message}")
        }
    }

    // -----------------------------------------------------------------------
    // Audio Focus
    // -----------------------------------------------------------------------

    private fun requestAudioFocus(): Boolean {
        val am = getSystemService(Context.AUDIO_SERVICE) as? AudioManager ?: return false
        audioManager = am
        return try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                val playbackAttributes = AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_MEDIA)
                    .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
                    .build()
                val handler = Handler(Looper.getMainLooper())
                val request = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN_TRANSIENT_MAY_DUCK)
                    .setAudioAttributes(playbackAttributes)
                    .setAcceptsDelayedFocusGain(false)
                    .setOnAudioFocusChangeListener({ /* transient ducking handled by system */ }, handler)
                    .build()
                audioFocusRequest = request
                val granted = am.requestAudioFocus(request) == AudioManager.AUDIOFOCUS_REQUEST_GRANTED
                Log.i("YapperRust", "Audio focus request granted=$granted")
                granted
            } else {
                @Suppress("DEPRECATION")
                val granted = am.requestAudioFocus(
                    null,
                    AudioManager.STREAM_MUSIC,
                    AudioManager.AUDIOFOCUS_GAIN_TRANSIENT_MAY_DUCK
                ) == AudioManager.AUDIOFOCUS_REQUEST_GRANTED
                Log.i("YapperRust", "Legacy audio focus request granted=$granted")
                granted
            }
        } catch (e: Throwable) {
            Log.w("YapperRust", "Failed to request audio focus: ${e.message}", e)
            false
        }
    }

    private fun abandonAudioFocus() {
        try {
            val am = audioManager ?: return
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                audioFocusRequest?.let { am.abandonAudioFocusRequest(it) }
                audioFocusRequest = null
            } else {
                @Suppress("DEPRECATION")
                am.abandonAudioFocus(null)
            }
        } catch (e: Exception) {
            Log.w(TAG, "Error abandoning audio focus: ${e.message}")
        }
    }

    // -----------------------------------------------------------------------
    // MediaSession
    // -----------------------------------------------------------------------

    private fun initMediaSession() {
        mediaSession = MediaSessionCompat(this, "YapperMediaSession").apply {
            isActive = true
            setCallback(object : MediaSessionCompat.Callback() {
                override fun onPlay() {
                    RustBridge.handleNotificationAction(ACTION_START_RESUME)
                }

                override fun onPause() {
                    stopAudio()
                    RustBridge.handleNotificationAction(ACTION_STOP_PAUSE)
                }

                override fun onSkipToNext() {
                    stopAudio()
                    RustBridge.handleNotificationAction(ACTION_SKIP)
                }

                override fun onStop() {
                    stopAudio()
                    RustBridge.handleNotificationAction(ACTION_STOP_PAUSE)
                }
            })

            setPlaybackState(
                PlaybackStateCompat.Builder()
                    .setActions(
                        PlaybackStateCompat.ACTION_PLAY or
                                PlaybackStateCompat.ACTION_PAUSE or
                                PlaybackStateCompat.ACTION_PLAY_PAUSE or
                                PlaybackStateCompat.ACTION_SKIP_TO_NEXT or
                                PlaybackStateCompat.ACTION_STOP
                    )
                    .setState(
                        if (isCurrentlyPaused) PlaybackStateCompat.STATE_PAUSED else PlaybackStateCompat.STATE_PLAYING,
                        0L,
                        1.0f
                    )
                    .build()
            )

            val metadata = MediaMetadataCompat.Builder()
                .putString(MediaMetadataCompat.METADATA_KEY_TITLE, currentTitle)
                .putString(MediaMetadataCompat.METADATA_KEY_ARTIST, currentSubtitle)
                .putString(MediaMetadataCompat.METADATA_KEY_DISPLAY_TITLE, currentTitle)
                .putString(MediaMetadataCompat.METADATA_KEY_DISPLAY_SUBTITLE, currentSubtitle)
                .build()
            setMetadata(metadata)
        }
    }

    // -----------------------------------------------------------------------
    // Notification & Channel
    // -----------------------------------------------------------------------

    private fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val channel = NotificationChannel(
                CHANNEL_ID,
                "Yapper Live TTS",
                NotificationManager.IMPORTANCE_DEFAULT
            ).apply {
                description = "TikTok LIVE background speech and player controls"
                setShowBadge(false)
                setSound(null, null)
                lockscreenVisibility = Notification.VISIBILITY_PUBLIC
            }
            val manager = getSystemService(NotificationManager::class.java)
            manager?.createNotificationChannel(channel)
        }
    }

    fun updateNotificationState(
        title: String,
        subtitle: String,
        queueDepth: Int,
        isPaused: Boolean,
        isConnected: Boolean
    ) {
        currentTitle = title
        currentSubtitle = subtitle
        currentQueueDepth = queueDepth
        isCurrentlyPaused = isPaused
        isCurrentlyConnected = isConnected

        mediaSession?.apply {
            setPlaybackState(
                PlaybackStateCompat.Builder()
                    .setActions(
                        PlaybackStateCompat.ACTION_PLAY or
                                PlaybackStateCompat.ACTION_PAUSE or
                                PlaybackStateCompat.ACTION_PLAY_PAUSE or
                                PlaybackStateCompat.ACTION_SKIP_TO_NEXT or
                                PlaybackStateCompat.ACTION_STOP
                    )
                    .setState(
                        if (isPaused) PlaybackStateCompat.STATE_PAUSED else PlaybackStateCompat.STATE_PLAYING,
                        0L,
                        1.0f
                    )
                    .build()
            )

            val metadata = MediaMetadataCompat.Builder()
                .putString(MediaMetadataCompat.METADATA_KEY_TITLE, title)
                .putString(MediaMetadataCompat.METADATA_KEY_ARTIST, subtitle)
                .putString(MediaMetadataCompat.METADATA_KEY_DISPLAY_TITLE, title)
                .putString(MediaMetadataCompat.METADATA_KEY_DISPLAY_SUBTITLE, subtitle)
                .build()
            setMetadata(metadata)
        }

        try {
            val notification = buildNotification()
            val manager = getSystemService(Context.NOTIFICATION_SERVICE) as? NotificationManager
            manager?.notify(NOTIFICATION_ID, notification)
        } catch (e: Throwable) {
            Log.e(TAG, "Failed to post notification: ${e.message}", e)
        }
    }

    private fun buildNotification(): Notification {
        val contentIntent = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java).apply {
                flags = Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP
            },
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        )

        // Action 1: Toggle Start/Resume or Pause/Stop
        val isPausedOrIdle = isCurrentlyPaused || !isCurrentlyConnected
        val toggleActionIntent = Intent(this, NotificationActionReceiver::class.java).apply {
            action = if (isPausedOrIdle) ACTION_START_RESUME else ACTION_STOP_PAUSE
        }
        val togglePendingIntent = PendingIntent.getBroadcast(
            this,
            1,
            toggleActionIntent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        )

        val toggleIcon = if (isPausedOrIdle) R.drawable.ic_play_arrow else R.drawable.ic_pause
        val toggleLabel = if (isPausedOrIdle) "Resume" else "Pause"

        // Action 2: Skip current message
        val skipIntent = Intent(this, NotificationActionReceiver::class.java).apply {
            action = ACTION_SKIP
        }
        val skipPendingIntent = PendingIntent.getBroadcast(
            this,
            2,
            skipIntent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        )

        val mediaStyle = MediaStyle()
            .setShowActionsInCompactView(0, 1)

        mediaSession?.sessionToken?.let { token ->
            mediaStyle.setMediaSession(token)
        }

        return NotificationCompat.Builder(this, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(currentTitle)
            .setContentText(currentSubtitle)
            .setSubText("Queue: $currentQueueDepth")
            .setContentIntent(contentIntent)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)
            .setStyle(mediaStyle)
            .addAction(toggleIcon, toggleLabel, togglePendingIntent)
            .addAction(R.drawable.ic_skip_next, "Skip", skipPendingIntent)
            .build()
    }

    // -----------------------------------------------------------------------
    // Native AudioTrack Playback with Immediate Interruption & Focus Ducking
    // -----------------------------------------------------------------------

    fun playAudio(pcmBytes: ByteArray, sampleRate: Int, seq: Long) {
        Log.i("YapperRust", "playAudio called: seq=$seq, bytes=${pcmBytes.size}, sampleRate=$sampleRate")
        isAudioCancelled.set(false)

        audioExecutor.execute {
            var track: AudioTrack? = null
            try {
                // Convert Mono 16-bit PCM to Stereo 16-bit PCM by duplicating samples.
                // Dual channels ensure loud hardware playback on stereo phones (Xiaomi/HyperOS).
                val stereoBytes = ByteArray(pcmBytes.size * 2)
                var sIdx = 0
                var maxAmp: Short = 0
                for (i in 0 until pcmBytes.size step 2) {
                    val b0 = pcmBytes[i]
                    val b1 = pcmBytes[i + 1]
                    val sample = ((b1.toInt() shl 8) or (b0.toInt() and 0xFF)).toShort()
                    val abs = if (sample < 0) (-sample).toShort() else sample
                    if (abs > maxAmp) maxAmp = abs

                    // Left
                    stereoBytes[sIdx++] = b0
                    stereoBytes[sIdx++] = b1
                    // Right
                    stereoBytes[sIdx++] = b0
                    stereoBytes[sIdx++] = b1
                }
                Log.i("YapperRust", "Converted to stereo: ${stereoBytes.size} bytes, max amplitude=$maxAmp")

                val minBufferSize = AudioTrack.getMinBufferSize(
                    sampleRate,
                    AudioFormat.CHANNEL_OUT_STEREO,
                    AudioFormat.ENCODING_PCM_16BIT
                )
                val bufferSize = if (minBufferSize > 0) maxOf(minBufferSize * 4, 16384) else 16384
                Log.i("YapperRust", "AudioTrack bufferSize=$bufferSize, minBufferSize=$minBufferSize")

                val attributes = AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_MEDIA)
                    .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
                    .build()

                val format = AudioFormat.Builder()
                    .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                    .setSampleRate(sampleRate)
                    .setChannelMask(AudioFormat.CHANNEL_OUT_STEREO)
                    .build()

                track = try {
                    AudioTrack.Builder()
                        .setAudioAttributes(attributes)
                        .setAudioFormat(format)
                        .setBufferSizeInBytes(bufferSize)
                        .setTransferMode(AudioTrack.MODE_STREAM)
                        .build()
                } catch (e: Throwable) {
                    Log.w("YapperRust", "AudioTrack.Builder failed, falling back to legacy constructor: ${e.message}")
                    @Suppress("DEPRECATION")
                    AudioTrack(
                        AudioManager.STREAM_MUSIC,
                        sampleRate,
                        AudioFormat.CHANNEL_OUT_STEREO,
                        AudioFormat.ENCODING_PCM_16BIT,
                        bufferSize,
                        AudioTrack.MODE_STREAM
                    )
                }

                Log.i("YapperRust", "AudioTrack created: state=${track.state} (INITIALIZED=${AudioTrack.STATE_INITIALIZED})")
                if (track.state != AudioTrack.STATE_INITIALIZED) {
                    Log.e("YapperRust", "AudioTrack FAILED to initialize!")
                    return@execute
                }

                track.setVolume(1.0f)
                activeAudioTrack = track

                requestAudioFocus()
                track.play()
                Log.i("YapperRust", "AudioTrack play() called. playState=${track.playState}")

                // Stream stereo PCM in chunks of 4096 bytes
                val sliceSize = 4096
                var offset = 0
                var totalWritten = 0
                while (offset < stereoBytes.size && !isAudioCancelled.get()) {
                    val bytesToWrite = minOf(sliceSize, stereoBytes.size - offset)
                    val written = track.write(stereoBytes, offset, bytesToWrite)
                    if (written < 0) {
                        Log.e("YapperRust", "AudioTrack.write error code: $written")
                        break
                    }
                    offset += written
                    totalWritten += written
                }
                Log.i("YapperRust", "AudioTrack wrote $totalWritten / ${stereoBytes.size} bytes")

                // Wait for all written frames to drain through the physical DAC
                // 1 stereo 16-bit frame = 4 bytes (2 bytes L + 2 bytes R)
                val totalFrames = totalWritten / 4
                val durationMs = (totalFrames * 1000L) / sampleRate
                val maxWaitMs = durationMs + 800L
                val startTime = System.currentTimeMillis()
                Log.i("YapperRust", "Waiting for drain: totalFrames=$totalFrames, durationMs=${durationMs}ms, maxWaitMs=${maxWaitMs}ms")

                while (!isAudioCancelled.get() && track.playState == AudioTrack.PLAYSTATE_PLAYING) {
                    val head = track.playbackHeadPosition
                    if (head >= totalFrames) {
                        Log.i("YapperRust", "Drain complete: head=$head reached totalFrames=$totalFrames")
                        break
                    }
                    val elapsed = System.currentTimeMillis() - startTime
                    if (elapsed > maxWaitMs) {
                        Log.i("YapperRust", "Drain grace timeout reached (${elapsed}ms, head=$head / $totalFrames)")
                        break
                    }
                    try {
                        Thread.sleep(25)
                    } catch (_: InterruptedException) {
                        break
                    }
                }
            } catch (e: Throwable) {
                Log.e("YapperRust", "Exception in playAudio: ${e.message}", e)
            } finally {
                Log.i("YapperRust", "Cleaning up track for seq=$seq")
                try {
                    if (isAudioCancelled.get()) {
                        track?.pause()
                        track?.flush()
                    }
                    track?.stop()
                    track?.release()
                } catch (e: Throwable) {
                    Log.w("YapperRust", "Error releasing track: ${e.message}")
                }
                activeAudioTrack = null
                abandonAudioFocus()
                Log.i("YapperRust", "Calling RustBridge.onAudioCompleted(seq=$seq)")
                RustBridge.onAudioCompleted(seq)
            }
        }
    }

    fun stopAudio() {
        Log.i("YapperRust", "stopAudio called")
        isAudioCancelled.set(true)
        try {
            activeAudioTrack?.let {
                it.pause()
                it.flush()
                it.stop()
            }
        } catch (e: Exception) {
            Log.w("YapperRust", "Error stopping active AudioTrack: ${e.message}")
        }
    }
}
