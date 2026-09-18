package com.splode.pomotroid

import android.Manifest
import android.app.Activity
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.media.AudioAttributes
import android.media.MediaPlayer
import android.os.Build
import android.util.Log
import androidx.core.content.ContextCompat
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.JSObject
import app.tauri.plugin.Invoke
import app.tauri.plugin.Plugin
import java.io.File

/**
 * Bridges the Rust timer to Android's native layer.
 *
 * Rust calls (see `src-tauri/src/mobile/timer_service.rs`):
 *   update        — start/refresh/stop the foreground service + countdown notification
 *   roundChanged  — heads-up notification for a new round
 *   playSound     — play an audio file with MediaPlayer (works while backgrounded)
 *   stop          — tear the service down
 *
 * Notification actions travel the other way: TimerForegroundService broadcasts
 * them, and this plugin forwards them to Rust as the `timer-service://action`
 * event, which maps onto TimerController (or the paired desktop in remote mode).
 */
@InvokeArg
class UpdateArgs {
    var roundType: String = "work"
    var elapsedSecs: Int = 0
    var totalSecs: Int = 0
    var isRunning: Boolean = false
    var isPaused: Boolean = false
    var roundNumber: Int = 1
    var roundsTotal: Int = 4
}

@InvokeArg
class RoundChangedArgs {
    var roundType: String = "work"
    var title: String = ""
    var body: String = ""
}

@InvokeArg
class PlaySoundArgs {
    var path: String = ""
    var volume: Float = 1.0f
}

@TauriPlugin
class TimerServicePlugin(private val activity: Activity) : Plugin(activity) {
    private var player: MediaPlayer? = null

    override fun load(webView: android.webkit.WebView) {
        super.load(webView)
        TimerActionBus.listener = { action ->
            val payload = JSObject()
            payload.put("action", action)
            trigger("action", payload)
        }
    }

    private fun notificationsAllowed(): Boolean {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) return true
        return ContextCompat.checkSelfPermission(activity, Manifest.permission.POST_NOTIFICATIONS) ==
            PackageManager.PERMISSION_GRANTED
    }

    /** Start, refresh or stop the foreground service for the current timer state. */
    @Command
    fun update(invoke: Invoke) {
        val args = invoke.parseArgs(UpdateArgs::class.java)
        try {
            val active = args.isRunning || args.isPaused
            if (!active) {
                activity.stopService(Intent(activity, TimerForegroundService::class.java))
            } else if (notificationsAllowed()) {
                val intent = Intent(activity, TimerForegroundService::class.java).apply {
                    action = TimerForegroundService.ACTION_UPDATE
                    putExtra(TimerForegroundService.EXTRA_ROUND_TYPE, args.roundType)
                    putExtra(TimerForegroundService.EXTRA_ELAPSED, args.elapsedSecs)
                    putExtra(TimerForegroundService.EXTRA_TOTAL, args.totalSecs)
                    putExtra(TimerForegroundService.EXTRA_RUNNING, args.isRunning)
                    putExtra(TimerForegroundService.EXTRA_PAUSED, args.isPaused)
                    putExtra(TimerForegroundService.EXTRA_ROUND_NUMBER, args.roundNumber)
                    putExtra(TimerForegroundService.EXTRA_ROUNDS_TOTAL, args.roundsTotal)
                }
                ContextCompat.startForegroundService(activity, intent)
            }
            invoke.resolve()
        } catch (e: Exception) {
            Log.w(TAG, "update failed", e)
            invoke.reject(e.message ?: "update failed")
        }
    }

    /** Post a heads-up notification announcing the new round. */
    @Command
    fun roundChanged(invoke: Invoke) {
        val args = invoke.parseArgs(RoundChangedArgs::class.java)
        try {
            if (notificationsAllowed()) {
                TimerNotifications.ensureChannels(activity)
                TimerNotifications.postRoundAlert(activity, args.roundType, args.title, args.body)
            }
            invoke.resolve()
        } catch (e: Exception) {
            Log.w(TAG, "roundChanged failed", e)
            invoke.reject(e.message ?: "roundChanged failed")
        }
    }

    /** Play a local audio file. Replaces any sound still playing. */
    @Command
    fun playSound(invoke: Invoke) {
        val args = invoke.parseArgs(PlaySoundArgs::class.java)
        try {
            val file = File(args.path)
            if (!file.exists()) {
                invoke.reject("sound file not found: ${args.path}")
                return
            }
            player?.let {
                try { it.stop() } catch (_: Exception) {}
                it.release()
            }
            val mp = MediaPlayer().apply {
                setAudioAttributes(
                    AudioAttributes.Builder()
                        .setUsage(AudioAttributes.USAGE_ALARM)
                        .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
                        .build()
                )
                setDataSource(file.absolutePath)
                val v = args.volume.coerceIn(0f, 1f)
                setVolume(v, v)
                setOnCompletionListener { p ->
                    p.release()
                    if (player === p) player = null
                }
                prepare()
                start()
            }
            player = mp
            invoke.resolve()
        } catch (e: Exception) {
            Log.w(TAG, "playSound failed", e)
            invoke.reject(e.message ?: "playSound failed")
        }
    }

    /** Stop the foreground service and any sound. */
    @Command
    fun stop(invoke: Invoke) {
        try {
            activity.stopService(Intent(activity, TimerForegroundService::class.java))
            player?.let {
                try { it.stop() } catch (_: Exception) {}
                it.release()
            }
            player = null
            invoke.resolve()
        } catch (e: Exception) {
            invoke.reject(e.message ?: "stop failed")
        }
    }

    companion object {
        const val TAG = "TimerServicePlugin"
    }
}

/** Delivers notification-action taps from the service to the plugin instance. */
object TimerActionBus {
    var listener: ((String) -> Unit)? = null

    fun emit(context: Context, action: String) {
        val l = listener
        if (l != null) {
            l(action)
        } else {
            Log.w(TimerServicePlugin.TAG, "no listener for action $action")
        }
    }
}
