package com.splode.pomotroid

import android.Manifest
import android.app.Activity
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
            releasePlayer()
            val mp = MediaPlayer()
            try {
                mp.setAudioAttributes(
                    AudioAttributes.Builder()
                        .setUsage(AudioAttributes.USAGE_ALARM)
                        .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
                        .build()
                )
                mp.setDataSource(file.absolutePath)
                val v = args.volume.coerceIn(0f, 1f)
                mp.setVolume(v, v)
                mp.setOnCompletionListener { p ->
                    p.release()
                    if (player === p) player = null
                }
                mp.prepare()
                mp.start()
            } catch (e: Exception) {
                // A file the decoder rejects must not leave a native player
                // behind: MediaPlayer holds codec resources until release().
                mp.release()
                throw e
            }
            player = mp
            invoke.resolve()
        } catch (e: Exception) {
            Log.w(TAG, "playSound failed", e)
            invoke.reject(e.message ?: "playSound failed")
        }
    }

    /**
     * Hand Rust the notification-action taps queued since the last call, as a
     * comma-separated list. Rust polls this because Tauri's `trigger()` only
     * reaches the webview, which is paused while the app is in the background.
     */
    @Command
    fun takeActions(invoke: Invoke) {
        val ret = JSObject()
        ret.put("actions", TimerActionBus.drain().joinToString(","))
        invoke.resolve(ret)
    }

    /** Stop the foreground service and any sound. */
    @Command
    fun stop(invoke: Invoke) {
        try {
            activity.stopService(Intent(activity, TimerForegroundService::class.java))
            releasePlayer()
            invoke.resolve()
        } catch (e: Exception) {
            invoke.reject(e.message ?: "stop failed")
        }
    }

    /** Stop and free the current player, and forget it, so a later failure
     *  never finds a released instance still referenced here. */
    private fun releasePlayer() {
        val p = player ?: return
        player = null
        try { p.stop() } catch (_: Exception) {}
        p.release()
    }

    companion object {
        const val TAG = "TimerServicePlugin"
    }
}

/**
 * Queue of notification-action taps, filled by TimerForegroundService and
 * drained by Rust through the `takeActions` command.
 */
object TimerActionBus {
    private val pending = java.util.concurrent.ConcurrentLinkedQueue<String>()

    fun emit(action: String) {
        pending.add(action)
        Log.i(TimerServicePlugin.TAG, "queued action $action")
    }

    fun drain(): List<String> {
        val out = mutableListOf<String>()
        while (true) {
            out.add(pending.poll() ?: break)
        }
        return out
    }
}
