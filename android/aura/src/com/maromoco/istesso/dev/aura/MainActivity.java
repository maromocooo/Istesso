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
 * The first screen: the host's address, pairing with it, and connecting. Built in code, without
 * layouts or libraries. The pairing code is taken from its field once and the field is emptied;
 * it is never kept, logged or shown again.
 */
public final class MainActivity extends Activity {
    private Settings settings;
    private EditText host;
    private EditText picturePort;
    private EditText inputPort;
    private EditText code;
    private TextView status;
    private Button pair;

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

        host = field(R.string.host_hint, InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_URI
                | InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS);
        host.setText(settings.host());
        column.addView(host);

        picturePort = field(R.string.picture_port_hint, InputType.TYPE_CLASS_NUMBER);
        picturePort.setText(String.valueOf(settings.picturePort()));
        column.addView(picturePort);

        inputPort = field(R.string.input_port_hint, InputType.TYPE_CLASS_NUMBER);
        inputPort.setText(String.valueOf(settings.inputPort()));
        column.addView(inputPort);

        // The code has letters and digits (ABCD-EFGH), so it is a text field; shown while typed (the owner types it
        // with the headset's own on-screen keyboard and must see it), in capitals, without suggestions.
        code = field(R.string.code_hint, InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD
                | InputType.TYPE_TEXT_FLAG_CAP_CHARACTERS | InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS);
        code.setImportantForAutofill(View.IMPORTANT_FOR_AUTOFILL_NO);
        code.setSaveEnabled(false);
        column.addView(code);

        pair = new Button(this);
        pair.setText(R.string.pair);
        pair.setOnClickListener(v -> pairNow());
        column.addView(pair);

        // The host's applications, one panel each (the owner's decision, 2026-11-18). Choosing one
        // that has a panel already brings that panel to the front.
        TextView apps = new TextView(this);
        apps.setText(R.string.apps_title);
        apps.setPadding(0, pad / 2, 0, 0);
        column.addView(apps);
        for (HostApp app : HostApp.ALL) {
            Button open = new Button(this);
            open.setText(app.titleRes);
            open.setOnClickListener(v -> openApp(app));
            column.addView(open);
        }

        status = new TextView(this);
        status.setPadding(0, pad / 2, 0, 0);
        column.addView(status);

        ScrollView scroll = new ScrollView(this);
        scroll.addView(column);
        setContentView(scroll);
        showPaired();
    }

    private EditText field(int hint, int type) {
        EditText e = new EditText(this);
        e.setHint(hint);
        e.setInputType(type);
        e.setSingleLine(true);
        return e;
    }

    private boolean saveHost() {
        String h = host.getText().toString().trim();
        if (h.isEmpty()) {
            status.setText(R.string.need_host);
            return false;
        }
        settings.setHost(h, port(picturePort, Settings.PICTURE_PORT), port(inputPort, Settings.INPUT_PORT));
        return true;
    }

    private static int port(EditText e, int otherwise) {
        try {
            int p = Integer.parseInt(e.getText().toString().trim());
            return p > 0 && p < 65536 ? p : otherwise;
        } catch (NumberFormatException ex) {
            return otherwise;
        }
    }

    private void showPaired() {
        String name = settings.hostName();
        status.setText(name.isEmpty() ? getString(R.string.not_paired) : getString(R.string.paired_with, name));
    }

    private void pairNow() {
        if (!saveHost()) {
            return;
        }
        // Taken once, and the field emptied at once.
        final String typed = code.getText().toString();
        code.getText().clear();
        final String address = settings.host();
        final String dir = settings.configDir;
        pair.setEnabled(false);
        status.setText(R.string.pairing);
        new Thread(() -> {
            String[] joined = Native.pairJoin(dir, address, typed, Settings.DEVICE_NAME);
            String why = joined == null ? Native.lastError(Native.ERROR_PAIRING) : null;
            runOnUiThread(() -> {
                pair.setEnabled(true);
                if (joined != null && joined.length == 2) {
                    settings.setPaired(joined[0], joined[1]);
                    showPaired();
                } else {
                    status.setText(getString(R.string.pair_failed, why == null ? "" : why));
                }
            });
        }, "istesso-pair").start();
    }

    /**
     * Opens the application's panel, or brings its panel to the front: a document of its own,
     * identified by the application (StreamActivity is documentLaunchMode="intoExisting"). Never
     * FLAG_ACTIVITY_MULTIPLE_TASK, which would add a panel every time.
     */
    private void openApp(HostApp app) {
        if (!saveHost()) {
            return;
        }
        if (settings.hostId().isEmpty()) {
            status.setText(R.string.need_pair);
            return;
        }
        Intent i = new Intent(this, StreamActivity.class);
        i.setData(app.uri());
        i.addFlags(Intent.FLAG_ACTIVITY_NEW_DOCUMENT | Intent.FLAG_ACTIVITY_LAUNCH_ADJACENT);
        startActivity(i);
    }
}
