// PROTOTYPE, for home use only (docs/m8-aura-prototype.md).
package com.maromoco.istesso.dev.aura;

import android.content.Context;
import android.net.Uri;

import java.util.ArrayList;
import java.util.List;

/**
 * The host applications the prototype offers, each in a panel of its own (the owner's decision,
 * 2026-11-18: one panel per host application).
 *
 * <p>The Ubuntu host: the four the trial edition opens for AURA (`scripts/daily.sh open NAME --for
 * aura`), each in a fixed AURA slot (trial-daily 78a6c10): editor 1, firefox 2, blender 3, chrome 4.
 *
 * <p>The Mac host: any application, in the slot it was opened in (`scripts/daily-mac.sh open APP
 * --for aura --slot N`, N 1 to 3); the panel is named by its slot.
 *
 * <p>On both, slot N has the picture on 4793N and the input on 4794N. So each panel knows where its
 * own application is without asking, and several panels can be open at once.
 */
final class HostApp {
    final Host host;
    final String name;
    final int titleRes;
    final int slot;

    private HostApp(Host host, String name, int titleRes, int slot) {
        this.host = host;
        this.name = name;
        this.titleRes = titleRes;
        this.slot = slot;
    }

    static final HostApp[] ALL = {
        new HostApp(Host.UBUNTU, "editor", R.string.app_editor, 1),
        new HostApp(Host.UBUNTU, "firefox", R.string.app_firefox, 2),
        new HostApp(Host.UBUNTU, "blender", R.string.app_blender, 3),
        new HostApp(Host.UBUNTU, "chrome", R.string.app_chrome, 4),
        new HostApp(Host.MAC, "slot-1", R.string.app_mac_slot, 1),
        new HostApp(Host.MAC, "slot-2", R.string.app_mac_slot, 2),
        new HostApp(Host.MAC, "slot-3", R.string.app_mac_slot, 3),
    };

    static List<HostApp> of(Host host) {
        List<HostApp> list = new ArrayList<>();
        for (HostApp a : ALL) {
            if (a.host == host) {
                list.add(a);
            }
        }
        return list;
    }

    static HostApp named(Host host, String name) {
        for (HostApp a : ALL) {
            if (a.host == host && a.name.equals(name)) {
                return a;
            }
        }
        return null;
    }

    /** What the person reads: the application, or the Mac's slot. */
    String title(Context context) {
        return context.getString(titleRes, slot);
    }

    /** The host and the application, as the panel names itself. */
    String fullTitle(Context context) {
        return context.getString(host.labelRes) + " — " + title(context);
    }

    /** For the log: host/name. Never an address. */
    String tag() {
        return host.key + "/" + name;
    }

    /**
     * The panel's identity: one document per host and application. Opening the same one again
     * finds the task with this identity and brings it to the front (documentLaunchMode
     * intoExisting) instead of adding a panel.
     */
    Uri uri() {
        return new Uri.Builder().scheme("istesso-aura").authority("app").appendPath(host.key).appendPath(name).build();
    }

    static HostApp of(Uri uri) {
        if (uri == null || !"istesso-aura".equals(uri.getScheme())) {
            return null;
        }
        List<String> path = uri.getPathSegments();
        if (path.size() == 1) {
            return named(Host.UBUNTU, path.get(0)); // a panel from before there were two hosts
        }
        if (path.size() == 2) {
            Host host = Host.named(path.get(0));
            return host == null ? null : named(host, path.get(1));
        }
        return null;
    }

    /** This application's picture on the host: 4793N for slot N. */
    int picturePort() {
        return 47930 + slot;
    }

    /** This application's input (the agent of its slot): 4794N for slot N. */
    int inputPort() {
        return 47940 + slot;
    }
}
