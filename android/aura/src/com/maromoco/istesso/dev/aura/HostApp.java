// PROTOTYPE, for home use only (docs/m8-aura-prototype.md).
package com.maromoco.istesso.dev.aura;

import android.net.Uri;

/**
 * The host applications the prototype offers, each in a panel of its own (the owner's decision,
 * 2026-11-18: one panel per host application). The four the trial edition opens for AURA
 * (`scripts/daily.sh open NAME --for aura`); Chrome since its crash report and usage statistics
 * are off in the trial (2026-11-19).
 *
 * <p>Ports: each application has a fixed AURA slot on the host (the trial edition, trial-daily
 * 78a6c10): editor 1, firefox 2, blender 3, chrome 4; the picture on 4793N and the input on
 * 4794N. So each panel knows where its own application is without asking, and several panels can
 * be open at once (the host runs at most three AURA applications at a time and refuses a fourth).
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
        new HostApp("firefox", R.string.app_firefox, 2),
        new HostApp("blender", R.string.app_blender, 3),
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

    /** This application's picture on the host: 4793N for slot N. */
    int picturePort() {
        return 47930 + slot;
    }

    /** This application's input (the agent of its slot): 4794N for slot N. */
    int inputPort() {
        return 47940 + slot;
    }
}
