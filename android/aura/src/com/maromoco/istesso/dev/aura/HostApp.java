// PROTOTYPE, for home use only (docs/m8-aura-prototype.md).
package com.maromoco.istesso.dev.aura;

import android.net.Uri;

/**
 * The host applications the prototype offers, each in a panel of its own (the owner's decision,
 * 2026-11-18: one panel per host application). The four the trial edition opens for AURA
 * (`scripts/daily.sh open NAME --for aura`); Chrome since its crash report and usage statistics
 * are off in the trial (2026-11-19).
 *
 * <p>Ports: the host has ONE slot for AURA for now (picture 47914, input 47924; a second
 * application is refused there while one runs), so every panel connects to the ports on the first
 * screen, whose defaults are those. A panel cannot tell which application the host shows in that
 * slot: open the panel of the application that was opened for AURA. Several panels at once need
 * several AURA slots on the host (docs/m8-aura-prototype.md, 7.2).
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
        new HostApp("chrome", R.string.app_chrome, 4),
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

    /** The host's one AURA slot for every application, for now. */
    int picturePort(Settings settings) {
        return settings.picturePort();
    }

    int inputPort(Settings settings) {
        return settings.inputPort();
    }
}
