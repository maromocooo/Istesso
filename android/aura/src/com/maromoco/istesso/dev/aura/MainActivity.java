// PROTOTYPE, for home use only (docs/m8-aura-prototype.md).
package com.maromoco.istesso.dev.aura;

import android.app.Activity;
import android.content.Intent;
import android.os.Bundle;
import android.text.InputType;
import android.view.View;
import android.widget.Button;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;

/**
 * The first screen: for each host (Host: Ubuntu, Mac), its address, pairing with it, and its
 * applications. Built in code, without layouts or libraries. A pairing code is taken from its
 * field once and the field is emptied; it is never kept, logged or shown again.
 */
public final class MainActivity extends Activity {
    private Settings settings;

    /** One host's part of the screen. */
    private final class Section {
        final Host host;
        EditText address;
        EditText code;
        TextView status;
        Button pair;

        Section(Host host) {
            this.host = host;
        }
    }

    @Override
    protected void onCreate(Bundle saved) {
        super.onCreate(saved);
        settings = new Settings(this);

        LinearLayout column = new LinearLayout(this);
        column.setOrientation(LinearLayout.VERTICAL);
        int pad = (int) (24 * getResources().getDisplayMetrics().density);
        column.setPadding(pad, pad, pad, pad);

        TextView mark = new TextView(this);
        mark.setText(getString(R.string.app_name) + "\n" + getString(R.string.mark));
        mark.setTextSize(22);
        column.addView(mark);

        TextView about = new TextView(this);
        about.setText(getString(R.string.about) + "\n" + getString(R.string.mark) + "  (" + BuildConfig.VERSION + ")");
        about.setPadding(0, pad / 2, 0, pad / 2);
        column.addView(about);

        for (Host host : Host.ALL) {
            addSection(column, new Section(host), pad);
        }

        ScrollView scroll = new ScrollView(this);
        scroll.addView(column);
        setContentView(scroll);
    }

    private void addSection(LinearLayout column, Section s, int pad) {
        TextView title = new TextView(this);
        title.setText(s.host.labelRes);
        title.setTextSize(20);
        title.setPadding(0, pad, 0, pad / 4);
        column.addView(title);

        s.address = field(R.string.host_hint, InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_URI
                | InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS);
        s.address.setText(settings.host(s.host));
        column.addView(s.address);

        // The code has letters and digits (ABCD-EFGH), so it is a text field; shown while typed (the owner types it
        // with the headset's own on-screen keyboard and must see it), in capitals, without suggestions.
        s.code = field(R.string.code_hint, InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD
                | InputType.TYPE_TEXT_FLAG_CAP_CHARACTERS | InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS);
        s.code.setImportantForAutofill(View.IMPORTANT_FOR_AUTOFILL_NO);
        s.code.setSaveEnabled(false);
        column.addView(s.code);

        s.pair = new Button(this);
        s.pair.setText(R.string.pair);
        s.pair.setOnClickListener(v -> pairNow(s));
        column.addView(s.pair);

        s.status = new TextView(this);
        column.addView(s.status);
        showPaired(s);

        Button unpin = new Button(this);
        unpin.setText(R.string.unpin);
        unpin.setOnClickListener(v -> unpin(s));
        column.addView(unpin);

        // The host's applications, one panel each (the owner's decision, 2026-11-18). Choosing one
        // that has a panel already brings that panel to the front.
        TextView apps = new TextView(this);
        apps.setText(s.host == Host.MAC ? R.string.apps_title_mac : R.string.apps_title);
        apps.setPadding(0, pad / 2, 0, 0);
        column.addView(apps);
        for (HostApp app : HostApp.of(s.host)) {
            Button open = new Button(this);
            open.setText(app.title(this));
            open.setOnClickListener(v -> openApp(s, app));
            column.addView(open);
        }
    }

    private EditText field(int hint, int type) {
        EditText e = new EditText(this);
        e.setHint(hint);
        e.setInputType(type);
        e.setSingleLine(true);
        return e;
    }

    private boolean saveHost(Section s) {
        String h = s.address.getText().toString().trim();
        if (h.isEmpty()) {
            s.status.setText(R.string.need_host);
            return false;
        }
        settings.setHost(s.host, h);
        return true;
    }

    private void showPaired(Section s) {
        String name = settings.hostName(s.host);
        s.status.setText(name.isEmpty() ? getString(R.string.not_paired) : getString(R.string.paired_with, name));
    }

    private void pairNow(Section s) {
        if (!saveHost(s)) {
            return;
        }
        // Taken once, and the field emptied at once.
        final String typed = s.code.getText().toString();
        s.code.getText().clear();
        final String address = settings.host(s.host);
        final String dir = settings.configDir;
        s.pair.setEnabled(false);
        s.status.setText(R.string.pairing);
        new Thread(() -> {
            String[] joined = Native.pairJoin(dir, address, typed, Settings.DEVICE_NAME);
            String why = joined == null ? Native.lastError(Native.ERROR_PAIRING) : null;
            runOnUiThread(() -> {
                s.pair.setEnabled(true);
                if (joined != null && joined.length == 2 && pairedElsewhere(s.host, joined[0])) {
                    // The same device for two hosts: both sections would show one host's applications.
                    s.status.setText(R.string.same_host_twice);
                } else if (joined != null && joined.length == 2 && !s.host.expectedName.equals(joined[1])) {
                    // A name other than the one this section's host gives itself: perhaps the other
                    // host's code was typed here. Pinned only when the person says so.
                    confirmPin(s, joined[0], joined[1]);
                } else if (joined != null && joined.length == 2) {
                    // Pinned for this host only: its panels connect to this device and no other.
                    settings.setPaired(s.host, joined[0], joined[1]);
                    showPaired(s);
                } else {
                    s.status.setText(getString(R.string.pair_failed, why == null ? "" : why));
                }
            });
        }, "istesso-pair").start();
    }

    /** Asks whether a device of an unexpected name is this section's host before it is pinned. */
    private void confirmPin(Section s, String id, String name) {
        new android.app.AlertDialog.Builder(this)
                .setTitle(R.string.pin_check_title)
                .setMessage(getString(R.string.pin_check, name, getString(s.host.labelRes), s.host.expectedName))
                .setPositiveButton(R.string.pin_check_yes, (d, w) -> {
                    settings.setPaired(s.host, id, name);
                    showPaired(s);
                })
                .setNegativeButton(R.string.pin_check_no, (d, w) -> s.status.setText(getString(R.string.pin_not_used, name)))
                .setCancelable(false)
                .show();
    }

    /** Lets go of the section's pinned device, after asking. */
    private void unpin(Section s) {
        String name = settings.hostName(s.host);
        if (name.isEmpty()) {
            s.status.setText(R.string.not_paired);
            return;
        }
        new android.app.AlertDialog.Builder(this)
                .setMessage(getString(R.string.unpin_check, name, getString(s.host.labelRes)))
                .setPositiveButton(R.string.unpin_yes, (d, w) -> {
                    settings.clearPaired(s.host);
                    showPaired(s);
                })
                .setNegativeButton(R.string.pin_check_no, null)
                .show();
    }

    /** Whether another host's section is paired with the device `id` already. */
    private boolean pairedElsewhere(Host host, String id) {
        for (Host other : Host.ALL) {
            if (other != host && id.equals(settings.hostId(other))) {
                return true;
            }
        }
        return false;
    }

    /**
     * Opens the application's panel, or brings its panel to the front: a document of its own,
     * identified by the host and the application (StreamActivity is documentLaunchMode="intoExisting").
     * Never FLAG_ACTIVITY_MULTIPLE_TASK, which would add a panel every time.
     */
    private void openApp(Section s, HostApp app) {
        if (!saveHost(s)) {
            return;
        }
        if (settings.hostId(s.host).isEmpty()) {
            s.status.setText(R.string.need_pair);
            return;
        }
        Intent i = new Intent(this, StreamActivity.class);
        i.setData(app.uri());
        i.addFlags(Intent.FLAG_ACTIVITY_NEW_DOCUMENT | Intent.FLAG_ACTIVITY_LAUNCH_ADJACENT);
        startActivity(i);
    }
}
