// PROTOTYPE, for home use only (docs/m8-aura-prototype.md).
package com.maromoco.istesso.dev.aura;

/**
 * The hosts the prototype knows, each with its own address and its own paired device (the owner,
 * 2026-11-19: AURA uses the applications of the Ubuntu host and of the Mac Studio). Both are paired
 * with the one identity this application has; each panel connects to the device of its own host
 * only (its id is pinned), never to whichever paired device answers.
 */
final class Host {
    /** The key in the settings and in a panel's identity. */
    final String key;
    final int labelRes;
    /**
     * Whether the host's trial agent takes committed text ({"kind":"text"}). One that does not
     * refuses the whole message, keys included, and the input connects again: the switch for
     * typing is not shown on its panels.
     */
    final boolean takesText;

    private Host(String key, int labelRes, boolean takesText) {
        this.key = key;
        this.labelRes = labelRes;
        this.takesText = takesText;
    }

    /** The Linux host (scripts/daily.sh open NAME --for aura). Its settings are the ones from before. */
    static final Host UBUNTU = new Host("ubuntu", R.string.host_ubuntu, true);
    /** The Mac host (scripts/daily-mac.sh open APP --for aura --slot N). */
    static final Host MAC = new Host("mac", R.string.host_mac, false); // until the Studio's trial agent takes text (m4-daily-host)

    static final Host[] ALL = {UBUNTU, MAC};

    static Host named(String key) {
        for (Host h : ALL) {
            if (h.key.equals(key)) {
                return h;
            }
        }
        return null;
    }
}
