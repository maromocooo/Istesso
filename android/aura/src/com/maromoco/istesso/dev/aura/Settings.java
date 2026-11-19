// PROTOTYPE, for home use only (docs/m8-aura-prototype.md).
package com.maromoco.istesso.dev.aura;

import android.content.Context;
import android.content.SharedPreferences;

import java.io.File;

/**
 * What the application remembers, all in its own storage, which no other application can read:
 * for each host (Host), its address and which paired device it is (each application's ports are fixed:
 * {@link HostApp}). The device's key and the list of
 * paired devices are in {@link #configDir}, kept by the Rust library as `istesso` keeps them.
 * The pairing code is never kept.
 */
final class Settings {
    static final String DEVICE_NAME = "aura-proto";

    private final SharedPreferences prefs;
    final String configDir;

    Settings(Context context) {
        prefs = context.getSharedPreferences("istesso-aura", Context.MODE_PRIVATE);
        File dir = new File(context.getFilesDir(), "istesso");
        if (!dir.isDirectory()) {
            dir.mkdirs();
        }
        // Only this application's user may enter it (the files inside are 0600 already).
        dir.setReadable(false, false);
        dir.setWritable(false, false);
        dir.setExecutable(false, false);
        dir.setReadable(true, true);
        dir.setWritable(true, true);
        dir.setExecutable(true, true);
        configDir = dir.getAbsolutePath();
    }

    /**
     * The settings' names for one host. The Ubuntu host keeps the names it had when there was one
     * host only, so that its address and pairing stay as they were.
     */
    private static String name(Host h, String what) {
        return h == Host.UBUNTU ? what : h.key + "." + what;
    }

    String host(Host h) { return prefs.getString(name(h, "host"), ""); }
    String hostId(Host h) { return prefs.getString(name(h, "host_id"), ""); }
    String hostName(Host h) { return prefs.getString(name(h, "host_name"), ""); }

    void setHost(Host h, String host) {
        prefs.edit().putString(name(h, "host"), host.trim()).apply();
    }

    void setPaired(Host h, String id, String peerName) {
        prefs.edit().putString(name(h, "host_id"), id).putString(name(h, "host_name"), peerName).apply();
    }

    /** "address:port", with brackets around an IPv6 address. */
    static String join(String host, int port) {
        String h = host.trim();
        if (h.contains(":") && !h.startsWith("[")) {
            h = "[" + h + "]";
        }
        return h + ":" + port;
    }
}
