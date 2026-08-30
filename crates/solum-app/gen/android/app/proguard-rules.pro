# Keep all Solum activity, listener and plugin classes/methods called via JNI and reflection
-keep class dev.solum.** {
    public *;
    protected *;
}

-keep class dev.solum.app.MainActivity {
    *;
}

-keep class dev.solum.app.SolumNotificationListener {
    *;
}

-keep class dev.solum.alarm.** {
    *;
}

-keep class dev.solum.healthconnect.** {
    *;
}

-keep class dev.solum.notifaccess.** {
    *;
}