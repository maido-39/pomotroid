package com.splode.pomotroid

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import android.os.SystemClock
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat

/** Notification channel ids and helpers shared by the service and the plugin. */
object TimerNotifications {
    const val CHANNEL_ONGOING = "pomotroid_timer"
    const val CHANNEL_ALERTS = "pomotroid_alerts"
    const val ONGOING_ID = 1001
    const val ALERT_ID = 1002

    fun ensureChannels(context: Context) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val manager = context.getSystemService(NotificationManager::class.java) ?: return

        val ongoing = NotificationChannel(
            CHANNEL_ONGOING,
            "Timer",
            NotificationManager.IMPORTANCE_LOW
        ).apply {
            description = "Shows the running Pomodoro timer"
            setShowBadge(false)
            setSound(null, null)
            enableVibration(false)
        }
        manager.createNotificationChannel(ongoing)

        val alerts = NotificationChannel(
            CHANNEL_ALERTS,
            "Round Alerts",
            NotificationManager.IMPORTANCE_HIGH
        ).apply {
            description = "Announces the start of each work round or break"
            // Sound is played by the app (MediaPlayer) so custom files work.
            setSound(null, null)
        }
        manager.createNotificationChannel(alerts)
    }

    fun roundLabel(roundType: String): String = when (roundType) {
        "work" -> "Focus"
        "short-break" -> "Short Break"
        else -> "Long Break"
    }

    fun contentIntent(context: Context): PendingIntent {
        val launch = context.packageManager.getLaunchIntentForPackage(context.packageName)
            ?: Intent(context, MainActivity::class.java)
        launch.addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP)
        return PendingIntent.getActivity(
            context,
            0,
            launch,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        )
    }

    fun postRoundAlert(context: Context, roundType: String, title: String, body: String) {
        ensureChannels(context)
        val notification = NotificationCompat.Builder(context, CHANNEL_ALERTS)
            .setSmallIcon(android.R.drawable.ic_popup_reminder)
            .setContentTitle(title.ifEmpty { roundLabel(roundType) })
            .setContentText(body)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setCategory(NotificationCompat.CATEGORY_ALARM)
            .setAutoCancel(true)
            .setContentIntent(contentIntent(context))
            .build()
        try {
            NotificationManagerCompat.from(context).notify(ALERT_ID, notification)
        } catch (_: SecurityException) {
            // POST_NOTIFICATIONS not granted — the in-app UI still updates.
        }
    }
}

/**
 * Keeps the Pomodoro timer alive and visible while the app is in the
 * background. The ongoing notification uses a chronometer so the remaining
 * time counts down without the service waking up every second.
 */
class TimerForegroundService : Service() {
    private var wakeLock: PowerManager.WakeLock? = null
    private var receiver: BroadcastReceiver? = null
    private var started = false

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        TimerNotifications.ensureChannels(this)
        receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) {
                val action = intent.getStringExtra(EXTRA_ACTION) ?: return
                TimerActionBus.emit(context, action)
            }
        }
        val filter = IntentFilter(ACTION_NOTIFICATION_ACTION)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            registerReceiver(receiver, filter, Context.RECEIVER_NOT_EXPORTED)
        } else {
            @Suppress("UnspecifiedRegisterReceiverFlag")
            registerReceiver(receiver, filter)
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent == null || intent.action != ACTION_UPDATE) {
            return START_STICKY
        }
        val roundType = intent.getStringExtra(EXTRA_ROUND_TYPE) ?: "work"
        val elapsed = intent.getIntExtra(EXTRA_ELAPSED, 0)
        val total = intent.getIntExtra(EXTRA_TOTAL, 0)
        val running = intent.getBooleanExtra(EXTRA_RUNNING, false)
        val paused = intent.getBooleanExtra(EXTRA_PAUSED, false)
        val roundNumber = intent.getIntExtra(EXTRA_ROUND_NUMBER, 1)
        val roundsTotal = intent.getIntExtra(EXTRA_ROUNDS_TOTAL, 4)

        val notification = buildNotification(roundType, elapsed, total, running, paused, roundNumber, roundsTotal)

        if (!started) {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
                startForeground(
                    TimerNotifications.ONGOING_ID,
                    notification,
                    ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE
                )
            } else {
                startForeground(TimerNotifications.ONGOING_ID, notification)
            }
            started = true
        } else {
            NotificationManagerCompat.from(this).notify(TimerNotifications.ONGOING_ID, notification)
        }

        if (running) acquireWakeLock() else releaseWakeLock()
        return START_STICKY
    }

    private fun buildNotification(
        roundType: String,
        elapsed: Int,
        total: Int,
        running: Boolean,
        paused: Boolean,
        roundNumber: Int,
        roundsTotal: Int,
    ): Notification {
        val remaining = (total - elapsed).coerceAtLeast(0)
        val label = TimerNotifications.roundLabel(roundType)
        val state = if (running) "$label · round $roundNumber/$roundsTotal" else "$label · paused"

        val builder = NotificationCompat.Builder(this, TimerNotifications.CHANNEL_ONGOING)
            .setSmallIcon(android.R.drawable.ic_lock_idle_alarm)
            .setContentTitle(state)
            .setOngoing(true)
            .setSilent(true)
            .setOnlyAlertOnce(true)
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .setCategory(NotificationCompat.CATEGORY_STOPWATCH)
            .setContentIntent(TimerNotifications.contentIntent(this))
            .setProgress(total.coerceAtLeast(1), elapsed, false)
            .addAction(0, if (running) "Pause" else "Resume", actionIntent("toggle"))
            .addAction(0, "Skip", actionIntent("skip"))

        if (running) {
            // Let the system count down without per-second service wakeups.
            builder
                .setUsesChronometer(true)
                .setChronometerCountDown(true)
                .setWhen(System.currentTimeMillis() + remaining * 1000L)
        } else {
            builder
                .setUsesChronometer(false)
                .setShowWhen(false)
                .setContentText(formatTime(remaining) + " left")
        }
        return builder.build()
    }

    private fun formatTime(seconds: Int): String {
        val m = seconds / 60
        val s = seconds % 60
        return String.format("%d:%02d", m, s)
    }

    private fun actionIntent(action: String): PendingIntent {
        val intent = Intent(ACTION_NOTIFICATION_ACTION)
            .setPackage(packageName)
            .putExtra(EXTRA_ACTION, action)
        return PendingIntent.getBroadcast(
            this,
            action.hashCode(),
            intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        )
    }

    private fun acquireWakeLock() {
        if (wakeLock?.isHeld == true) return
        val pm = getSystemService(Context.POWER_SERVICE) as PowerManager
        wakeLock = pm.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "pomotroid:timer").apply {
            setReferenceCounted(false)
            acquire(4 * 60 * 60 * 1000L)
        }
    }

    private fun releaseWakeLock() {
        wakeLock?.let { if (it.isHeld) it.release() }
        wakeLock = null
    }

    override fun onDestroy() {
        releaseWakeLock()
        receiver?.let { runCatching { unregisterReceiver(it) } }
        receiver = null
        started = false
        super.onDestroy()
    }

    companion object {
        const val ACTION_UPDATE = "com.splode.pomotroid.UPDATE"
        const val ACTION_NOTIFICATION_ACTION = "com.splode.pomotroid.NOTIFICATION_ACTION"
        const val EXTRA_ACTION = "action"
        const val EXTRA_ROUND_TYPE = "roundType"
        const val EXTRA_ELAPSED = "elapsed"
        const val EXTRA_TOTAL = "total"
        const val EXTRA_RUNNING = "running"
        const val EXTRA_PAUSED = "paused"
        const val EXTRA_ROUND_NUMBER = "roundNumber"
        const val EXTRA_ROUNDS_TOTAL = "roundsTotal"

        @Suppress("unused")
        fun unusedTimeBase() = SystemClock.elapsedRealtime()
    }
}
