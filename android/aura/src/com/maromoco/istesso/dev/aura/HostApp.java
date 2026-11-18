// PROTOTYPE, for home use only (docs/m8-aura-prototype.md).
package com.maromoco.istesso.dev.aura;

import android.net.Uri;

/**
 * The host applications the prototype offers, each in a panel of its own (the owner's decision,
 * 2026-11-18: one panel per host application). Only these three: Chrome stays out until its
 * crash-report and telemetry checks are done.
 *
 * <p>Ports: PROVISIONAL. Slot N has the picture on 4791N and the input's agent on 4792N, as the
 * trial edition numbers its slots. The host side is being brought into the trial edition
 * (`scripts/daily.sh`, a mode for AURA); when its interface is there, this follows it. "manual"
 * takes the two ports typed on the first screen (the AURA trial launcher's 47914 and 47924).
 */
final class HostApp {
    final String name;
    final int titleRes;
    final int slot;

    private HostApp(String name, int titleRes, int slot) {
        this.name = name;
        this.titleRes = titleRes;
        this.slot = slot;
    }

    static final HostApp[] ALL = {
        new HostApp("editor", R.string.app_editor, 1),
        new HostApp("blender", R.string.app_blender, 2),
        new HostApp("firefox", R.string.app_firefox, 3),
        new HostApp("manual", R.string.app_manual, 0),
    };

    static HostApp named(String name) {
        for (HostApp a : ALL) {
            if (a.name.equals(name)) {
                return a;
            }
        }
        return null;
    }

    /**
     * The panel's identity: one document per application. Opening the same application again
     * finds the task with this identity and brings it to the front (documentLaunchMode
     * intoExisting) instead of adding a panel.
     */
    Uri uri() {
        return new Uri.Builder().scheme("istesso-aura").authority("app").appendPath(name).build();
    }

    static HostApp of(Uri uri) {
        if (uri == null || !"istesso-aura".equals(uri.getScheme()) || uri.getPathSegments().size() != 1) {
            return null;
        }
        return named(uri.getPathSegments().get(0));
    }

    int picturePort(Settings settings) {
        return slot == 0 ? settings.picturePort() : 47910 + slot;
    }

    int inputPort(Settings settings) {
        return slot == 0 ? settings.inputPort() : 47920 + slot;
    }
}
