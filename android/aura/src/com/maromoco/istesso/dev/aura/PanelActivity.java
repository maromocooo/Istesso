package com.maromoco.istesso.dev.aura;

import android.app.Activity;
import android.content.Intent;
import android.graphics.Color;
import android.os.Bundle;
import android.view.Gravity;
import android.widget.Button;
import android.widget.LinearLayout;
import android.widget.TextView;

/**
 * A placeholder panel with no picture, for one question only (the owner, 2026-11-18): can Istesso show two or more
 * panels side by side in Home Space, one per host application? It connects to nothing and touches nothing on the
 * host. Each instance shows its number and can open one more.
 */
public final class PanelActivity extends Activity {
    static final String NUMBER = "number";
    static final String WAY = "way";

    /** Opens another placeholder panel as a task of its own, the way named by {@code way}. */
    static void open(Activity from, int number, int way) {
        Intent i = new Intent(from, PanelActivity.class);
        i.putExtra(NUMBER, number);
        i.putExtra(WAY, way);
        int flags = Intent.FLAG_ACTIVITY_MULTIPLE_TASK | Intent.FLAG_ACTIVITY_LAUNCH_ADJACENT;
        // A: a new task; B: a new document (another way Android has to give the same activity a task of its own).
        flags |= way == 0 ? Intent.FLAG_ACTIVITY_NEW_TASK : Intent.FLAG_ACTIVITY_NEW_DOCUMENT;
        i.addFlags(flags);
        from.startActivity(i);
    }

    @Override
    protected void onCreate(Bundle saved) {
        super.onCreate(saved);
        int number = getIntent().getIntExtra(NUMBER, 2);
        int way = getIntent().getIntExtra(WAY, 0);
        LinearLayout column = new LinearLayout(this);
        column.setOrientation(LinearLayout.VERTICAL);
        column.setGravity(Gravity.CENTER);
        column.setBackgroundColor(way == 0 ? 0xff1e3a5f : 0xff3f2a5f);
        TextView big = new TextView(this);
        big.setText(getString(R.string.panel_title, number, way == 0 ? "A" : "B"));
        big.setTextColor(Color.WHITE);
        big.setTextSize(36);
        big.setGravity(Gravity.CENTER);
        column.addView(big);
        TextView note = new TextView(this);
        note.setText(R.string.panel_note);
        note.setTextColor(Color.WHITE);
        note.setTextSize(16);
        note.setGravity(Gravity.CENTER);
        column.addView(note);
        Button more = new Button(this);
        more.setText(R.string.panel_more);
        more.setOnClickListener(v -> open(this, number + 1, way));
        column.addView(more);
        setContentView(column);
        setTitle(getString(R.string.panel_title, number, way == 0 ? "A" : "B"));
    }
}
