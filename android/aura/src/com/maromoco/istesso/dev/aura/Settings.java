// PROTOTYPE, for home use only (docs/m8-aura-prototype.md).
package com.maromoco.istesso.dev.aura;

import android.content.Context;
import android.content.SharedPreferences;

import java.io.File;

/**
 * What the application remembers, all in its own storage, which no other application can read:
 * the host's address, and which paired device it is (each application's ports are fixed:
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

    String host() { return prefs.getString("host", ""); }
    String hostId() { return prefs.getString("host_id", ""); }
    String hostName() { return prefs.getString("host_name", ""); }

    void setHost(String host) {
        prefs.edit().putString("host", host.trim()).remove("picture_port").remove("input_port").apply();
    }

    void setPaired(String id, String name) {
        prefs.edit().putString("host_id", id).putString("host_name", name).apply();
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
