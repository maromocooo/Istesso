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
     * The name the host gives itself when it shows a pairing code with the trial's own line
     * (`daily.sh aura-pair-line`: host-trial; `daily-mac.sh pair-line`: mac-trial). It is what the
     * host says, not something proven: the pairing carries only a name and a key. A pairing whose
     * name is not this one is pinned in this section only after the person confirms it.
     */
    final String expectedName;

    private Host(String key, int labelRes, String expectedName) {
        this.key = key;
        this.labelRes = labelRes;
        this.expectedName = expectedName;
    }

    /** The Linux host (scripts/daily.sh open NAME --for aura). Its settings are the ones from before. */
    static final Host UBUNTU = new Host("ubuntu", R.string.host_ubuntu, "host-trial");
    /** The Mac host (scripts/daily-mac.sh open APP --for aura --slot N). */
    static final Host MAC = new Host("mac", R.string.host_mac, "mac-trial");

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
