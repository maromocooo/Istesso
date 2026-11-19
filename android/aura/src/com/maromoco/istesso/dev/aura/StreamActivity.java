// PROTOTYPE, for home use only (docs/m8-aura-prototype.md).
package com.maromoco.istesso.dev.aura;

import android.app.Activity;
import android.content.Context;
import android.graphics.Color;
import android.net.wifi.WifiManager;
import android.os.Bundle;
import android.util.Log;
import android.view.Gravity;
import android.view.InputDevice;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.PointerIcon;
import android.view.SurfaceHolder;
import android.view.SurfaceView;
import android.view.View;
import android.view.ViewGroup;
import android.view.WindowManager;
import android.widget.FrameLayout;
import android.widget.TextView;

/**
 * One panel with the picture of one host application (the intent's data names it; one panel per
 * application, the owner's decision of 2026-11-18). Keys and the pointer made over it go to the host. While the
 * panel is in the background the picture is let go (the host gives its encoder back) and the
 * input is closed (the host lets go of whatever was held down).
 */
public final class StreamActivity extends Activity implements SurfaceHolder.Callback, Picture.Told {
    private static final String TAG = "istesso-aura";
    /** One notch of a wheel, in the pixels the host scrolls by (as the Mac client counts it). */
    private static final float WHEEL_PIXELS = 15f;

    private Settings settings;
    /** The host application this panel shows. */
    private HostApp app;
    private Fitted frame;
    private SurfaceView view;
    private TextView status;
    private String decoderLine = "";
    private String inputLine = "";
    private String pictureLine = "";
    /** The picture is not there (connecting, cut, or waiting to connect again): the status line stays. */
    private boolean away;
    /** How often in a row the input's connection was over; for the wait before it is opened again. */
    private int inputAttempt;

    private Picture picture;
    private boolean surfaceReady;
    private boolean resumed;
    private long input;
    private WifiManager.WifiLock wifiLock;

    private volatile int pictureWidth;
    private volatile int pictureHeight;
    private int buttons;
    private boolean touchDown;
    /** The owner's stand-in for a middle button (Blender orbits with it): while on, the left button goes as the middle one. */
    private boolean middle;
    private int touchButton = 0x110;
    private android.widget.Button middleToggle;
    /** Each kind of pointer event is written to the log once (tag IstessoAura), to learn what the puck's touchpad sends. */
    private final java.util.HashSet<String> seenKinds = new java.util.HashSet<>();
    private float scrollX;
    private float scrollY;

    private long statsSince;
    private long statsLines;

    /**
     * The battery, every 10 s in the stats line: level (%), charging or not, the current (µA as
     * the system says it; the sign differs between devices) and the temperature (°C). No
     * identifiers. Read from the system's last battery broadcast, without any permission.
     */
    private String batteryLine() {
        try {
            android.content.Intent b = registerReceiver(null,
                    new android.content.IntentFilter(android.content.Intent.ACTION_BATTERY_CHANGED));
            android.os.BatteryManager bm = (android.os.BatteryManager) getSystemService(Context.BATTERY_SERVICE);
            if (b == null || bm == null) {
                return " battery=unknown";
            }
            int level = b.getIntExtra(android.os.BatteryManager.EXTRA_LEVEL, -1);
            int scale = b.getIntExtra(android.os.BatteryManager.EXTRA_SCALE, 100);
            int status = b.getIntExtra(android.os.BatteryManager.EXTRA_STATUS, -1);
            int plugged = b.getIntExtra(android.os.BatteryManager.EXTRA_PLUGGED, 0);
            int tenths = b.getIntExtra(android.os.BatteryManager.EXTRA_TEMPERATURE, Integer.MIN_VALUE);
            boolean charging = status == android.os.BatteryManager.BATTERY_STATUS_CHARGING
                    || status == android.os.BatteryManager.BATTERY_STATUS_FULL;
            long current = bm.getIntProperty(android.os.BatteryManager.BATTERY_PROPERTY_CURRENT_NOW);
            return " battery_pct=" + (scale > 0 && level >= 0 ? level * 100 / scale : -1)
                    + " battery_charging=" + charging + " battery_plugged=" + plugged
                    + " battery_current_ua=" + current
                    + " battery_temp_c=" + (tenths == Integer.MIN_VALUE ? "?" : String.valueOf(tenths / 10.0));
        } catch (RuntimeException e) {
            return " battery=unknown";
        }
    }

    private final Runnable inputWatch = new Runnable() {
        @Override
        public void run() {
            showInput();
            logStats();
            if (resumed) {
                status.postDelayed(this, 1000);
            }
        }
    };

    /**
     * The Wi-Fi link: its speed (Mbit/s; transmit and receive where the system says them) and the
     * signal strength (dBm). Nothing else of the connection: never the network's name, the base
     * station's identity or an address. Needs no permission that asks the person (the network's
     * name would, and is not read).
     */
    @SuppressWarnings("deprecation")
    private String wifiLine() {
        try {
            WifiManager wifi = (WifiManager) getApplicationContext().getSystemService(Context.WIFI_SERVICE);
            android.net.wifi.WifiInfo w = wifi == null ? null : wifi.getConnectionInfo();
            if (w == null) {
                return " wifi=none";
            }
            return " wifi_mbit=" + w.getLinkSpeed() + " wifi_tx_mbit=" + w.getTxLinkSpeedMbps()
                    + " wifi_rx_mbit=" + w.getRxLinkSpeedMbps() + " wifi_rssi_dbm=" + w.getRssi();
        } catch (RuntimeException e) {
            return " wifi=unknown";
        }
    }

    /**
     * Once a second, one line for this panel (tag IstessoAura): the picture's counters and the
     * input's state. Sizes, counts and times only; never a key, a code or anything of the content.
     * The input library does not say how much is waiting to be sent, so that is not in the line.
     */
    private void logStats() {
        long now = System.nanoTime();
        long window = statsSince == 0 ? 1_000_000_000L : now - statsSince;
        statsSince = now;
        Picture p = picture;
        StringBuilder b = new StringBuilder("stats panel=").append(app.name).append(' ');
        b.append(p == null ? "picture=none" : p.statsLine(window));
        long[] s = input == 0 ? null : Native.inputState(input);
        if (s == null) {
            b.append(" input=none");
        } else {
            b.append(" input_open=").append(s[1]).append(" input_answered=").append(s[2])
                    .append(" input_cuts=").append(s[0]).append(" input_over=").append(s[3]);
        }
        b.append(wifiLine());
        if (++statsLines % 10 == 1) {
            b.append(batteryLine());
        }
        Log.i("IstessoAura", b.toString());
    }

    @Override
    protected void onCreate(Bundle saved) {
        super.onCreate(saved);
        settings = new Settings(this);
        app = HostApp.of(getIntent().getData());
        if (app == null) {
            app = HostApp.ALL[0];
        }
        setTitle(getString(R.string.panel_name, getString(app.titleRes)));
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);

        frame = new Fitted(this);
        frame.setBackgroundColor(Color.BLACK);
        view = new SurfaceView(this);
        view.getHolder().addCallback(this);
        view.setFocusable(true);
        view.setFocusableInTouchMode(true);
        // The host draws the pointer into the picture: the system's own is hidden over it.
        view.setPointerIcon(PointerIcon.getSystemIcon(this, PointerIcon.TYPE_NULL));
        view.setOnGenericMotionListener((v, e) -> generic(e));
        view.setOnTouchListener((v, e) -> touch(e));
        frame.addView(view, new FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.MATCH_PARENT, Gravity.CENTER));

        status = new TextView(this);
        status.setTextColor(Color.WHITE);
        status.setBackgroundColor(0x80000000);
        status.setTextSize(12);
        status.setPadding(8, 4, 8, 4);
        FrameLayout root = new FrameLayout(this);
        root.addView(frame, new FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.MATCH_PARENT));
        // Bottom right, where an app has the fewest buttons; it goes away 5 s after it last changed and comes back
        // when something changes (the owner, 2026-11-18: at the top left it covered the editor's buttons).
        middleToggle = new android.widget.Button(this);
        middleToggle.setAllCaps(false);
        middleToggle.setTextSize(12);
        middleToggle.setAlpha(0.7f);
        middleToggle.setText(getString(R.string.middle_off));
        middleToggle.setOnClickListener(v -> {
            middle = !middle;
            middleToggle.setText(getString(middle ? R.string.middle_on : R.string.middle_off));
        });
        root.addView(middleToggle, new FrameLayout.LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT,
                ViewGroup.LayoutParams.WRAP_CONTENT, Gravity.BOTTOM | Gravity.START));
        status.setClickable(false);
        status.setFocusable(false);
        root.addView(status, new FrameLayout.LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT,
                ViewGroup.LayoutParams.WRAP_CONTENT, Gravity.BOTTOM | Gravity.END));
        setContentView(root);
        line(getString(R.string.mark));

        WifiManager wifi = (WifiManager) getApplicationContext().getSystemService(Context.WIFI_SERVICE);
        if (wifi != null) {
            wifiLock = wifi.createWifiLock(WifiManager.WIFI_MODE_FULL_LOW_LATENCY, "istesso-aura");
            wifiLock.setReferenceCounted(false);
        }
    }

    /** The same application chosen again: this panel is brought to the front, nothing else. */
    @Override
    protected void onNewIntent(android.content.Intent intent) {
        super.onNewIntent(intent);
        HostApp asked = HostApp.of(intent.getData());
        Log.i(TAG, "panel brought to the front for " + (asked == null ? "?" : asked.name) + " (this panel: " + app.name + ")");
    }

    @Override
    protected void onResume() {
        super.onResume();
        resumed = true;
        inputAttempt = 0;
        openInput();
        statsSince = 0;
        status.removeCallbacks(inputWatch); // one loop, also when the panel comes back within a second
        status.post(inputWatch);
        view.requestFocus();
        startPicture();
    }

    @Override
    protected void onPause() {
        resumed = false;
        status.removeCallbacks(reopenInput);
        stopPicture();
        if (input != 0) {
            Native.inputClose(input);
            input = 0;
        }
        buttons = 0;
        touchDown = false;
        releaseWifi("panel in the background");
        super.onPause();
    }

    private void startPicture() {
        if (!resumed || !surfaceReady || picture != null) {
            return;
        }
        picture = new Picture(settings.configDir, Settings.join(settings.host(), app.picturePort()),
                settings.hostId(), view.getHolder().getSurface(), this);
        picture.start();
    }

    private void stopPicture() {
        if (picture != null) {
            picture.stop();
            picture = null;
        }
    }

    // ---- the surface ----

    @Override
    public void surfaceCreated(SurfaceHolder holder) {
        surfaceReady = true;
        startPicture();
    }

    @Override
    public void surfaceChanged(SurfaceHolder holder, int format, int width, int height) {}

    @Override
    public void surfaceDestroyed(SurfaceHolder holder) {
        surfaceReady = false;
        stopPicture();
    }

    // ---- what the picture tells (on its threads) ----

    @Override
    public void connecting() {
        runOnUiThread(() -> { away = true; pictureLine = getString(R.string.connecting); show(); });
    }

    @Override
    public void connected(int width, int height, int fps, int chroma) {
        pictureWidth = width;
        pictureHeight = height;
        runOnUiThread(() -> {
            away = false;
            takeWifi();
            frame.setAspect(width, height);
            pictureLine = getString(R.string.connected, width, height, fps, chroma);
            show();
        });
    }

    @Override
    public void cut(long cuts) {
        runOnUiThread(() -> { away = true; pictureLine = getString(R.string.cut, (int) cuts); show(); });
    }

    @Override
    public void reconnecting(int why, long waitMs, int attempt) {
        int text;
        switch (why) {
            case Picture.OVER_ENDED: text = R.string.again_ended; break;
            case Picture.OVER_SET_ASIDE: text = R.string.again_set_aside; break;
            case Picture.OVER_NOT_SHOWN: text = R.string.again_not_shown; break;
            default: text = R.string.again_no_answer; break;
        }
        runOnUiThread(() -> { away = true; pictureLine = getString(text); show(); });
    }

    @Override
    public void decoder(String name, boolean lowLatency) {
        Log.i(TAG, "decoder " + name + (lowLatency ? " (low latency)" : ""));
        runOnUiThread(() -> {
            decoderLine = getString(R.string.decoder, name, lowLatency ? getString(R.string.low_latency) : "");
            show();
        });
    }

    @Override
    public void over(String why) {
        String text;
        if ("no-decoder".equals(why)) {
            text = getString(R.string.no_decoder);
        } else if ("no-peer".equals(why)) {
            text = getString(R.string.need_pair);
        } else if (why != null && why.startsWith("unsupported:")) {
            int chroma = 0;
            try {
                chroma = Integer.parseInt(why.substring("unsupported:".length()));
            } catch (NumberFormatException ignored) {
                // stays 0
            }
            text = getString(R.string.unsupported, chroma);
        } else {
            text = getString(R.string.over, why == null ? "" : why);
        }
        runOnUiThread(() -> { away = false; pictureLine = text; show(); releaseWifi("picture over"); });
    }

    /**
     * Wi-Fi in its low-latency mode (WIFI_MODE_FULL_LOW_LATENCY) while this panel shows a picture.
     * Each panel holds its own lock; the system keeps the mode while any lock is held, so it ends
     * when no panel shows a picture. The system applies the mode only while the app is in front and
     * the screen is on; whether it did, and whether the device supports it, cannot be read through
     * the public SDK, so the log says what can be read: that the lock is held.
     */
    private void takeWifi() {
        if (wifiLock == null || wifiLock.isHeld()) {
            return;
        }
        wifiLock.acquire();
        // Whether the device supports the mode (WifiManager.isLowLatencyModeSupported) is not in the
        // public SDK this builds against, and whether the system applied it cannot be read either.
        Log.i("IstessoAura", "wifi lock taken panel=" + app.name + " mode=low-latency held=" + wifiLock.isHeld()
                + " applied=not-readable");
    }

    private void releaseWifi(String why) {
        if (wifiLock != null && wifiLock.isHeld()) {
            wifiLock.release();
            Log.i("IstessoAura", "wifi lock released panel=" + app.name + " (" + why + ")");
        }
    }

    private void showInput() {
        if (input == 0) {
            show();
            return;
        }
        long[] s = Native.inputState(input);
        if (s != null && s[3] != 0) {
            // Over for this connection: it is let go (the host lets go of whatever was held down)
            // and opened again after a wait, 0.25 s doubling up to 5 s, as the picture does.
            Native.inputClose(input);
            input = 0;
            buttons = 0;
            touchDown = false;
            inputAttempt++;
            long wait = Picture.againAfter(inputAttempt);
            Log.i("IstessoAura", "input: over; opening it again in " + wait + " ms");
            inputLine = getString(R.string.input_again);
            status.removeCallbacks(reopenInput);
            status.postDelayed(reopenInput, wait);
        } else if (s != null) {
            if (s[1] != 0) {
                inputAttempt = 0;
            }
            inputLine = s[1] != 0 ? getString(R.string.input_open) : getString(R.string.input_waiting);
        }
        show();
    }

    /** Opens the input's connection again, after it was over (see showInput). */
    private final Runnable reopenInput = new Runnable() {
        @Override
        public void run() {
            if (resumed && input == 0) {
                openInput();
                showInput();
            }
        }
    };

    private void openInput() {
        // Both connections name the paired host; without one, nothing is connected.
        String peer = settings.hostId();
        if (peer.trim().isEmpty()) {
            input = 0;
            inputLine = getString(R.string.need_pair);
        } else {
            input = Native.inputOpen(settings.configDir, peer, Settings.join(settings.host(), app.inputPort()));
            if (input == 0) {
                String why = Native.lastError(Native.ERROR_PAIRING);
                inputLine = getString(R.string.input_over) + " "
                        + (why.isEmpty() ? Native.lastError(Native.ERROR_INPUT) : why);
            }
        }
    }

    private final Runnable hideStatus = new Runnable() {
        @Override
        public void run() {
            status.setVisibility(View.INVISIBLE);
        }
    };

    private void line(String text) {
        if (text.contentEquals(status.getText())) {
            if (away) {
                status.removeCallbacks(hideStatus);
                status.setVisibility(View.VISIBLE);
            }
            return;
        }
        status.setText(text);
        status.setVisibility(View.VISIBLE);
        status.removeCallbacks(hideStatus);
        // While the picture is away the line stays: the panel says that it is connecting again.
        if (!away) {
            status.postDelayed(hideStatus, 5000);
        }
    }

    private void show() {
        StringBuilder b = new StringBuilder(getString(R.string.mark));
        for (String s : new String[] {pictureLine, decoderLine, inputLine}) {
            if (!s.isEmpty()) {
                b.append('\n').append(s);
            }
        }
        line(b.toString());
    }

    // ---- input ----

    private void send(int kind, int a, int b) {
        if (input != 0) {
            Native.inputSend(input, new int[] {kind, a, b}, 1);
        }
    }

    @Override
    public boolean dispatchKeyEvent(KeyEvent event) {
        int code = event.getKeyCode();
        if (KeyMap.forTheSystem(code) || input == 0) {
            return super.dispatchKeyEvent(event);
        }
        int linux = KeyMap.linux(event);
        if (linux == 0) {
            return super.dispatchKeyEvent(event);
        }
        int action = event.getAction();
        if (action == KeyEvent.ACTION_DOWN) {
            // The host repeats a key that is held; the system's repeats are not sent.
            if (event.getRepeatCount() == 0) {
                send(Native.INPUT_KEY, linux, 1);
            }
            return true;
        }
        if (action == KeyEvent.ACTION_UP) {
            send(Native.INPUT_KEY, linux, 0);
            return true;
        }
        return true;
    }

    /** Where the pointer is, in the picture's pixels; false when there is no picture yet. */
    private boolean place(MotionEvent e) {
        int w = pictureWidth;
        int h = pictureHeight;
        int vw = view.getWidth();
        int vh = view.getHeight();
        if (w <= 0 || h <= 0 || vw <= 0 || vh <= 0) {
            return false;
        }
        int x = (int) (e.getX() * w / vw);
        int y = (int) (e.getY() * h / vh);
        x = Math.max(0, Math.min(w - 1, x));
        y = Math.max(0, Math.min(h - 1, y));
        send(Native.INPUT_POINTER_AT, x, y);
        return true;
    }

    /** The buttons of a mouse that changed since last time, as BTN_LEFT, BTN_RIGHT, ... */
    private int left() {
        return middle ? 0x112 : 0x110;
    }

    private void noteKind(String where, MotionEvent e) {
        String k = where + " source=0x" + Integer.toHexString(e.getSource()) + " tool=" + e.getToolType(0)
                + " action=" + e.getActionMasked() + " class=" + e.getClassification() + " buttons=" + e.getButtonState()
                + " pointers=" + e.getPointerCount();
        if (seenKinds.add(k)) {
            android.util.Log.i("IstessoAura", k);
        }
    }

    /**
     * The scroll axes, written to the log once for each kind of device and direction (tag
     * IstessoAura), so that which way a two-finger swipe goes can be read next to what the host
     * did: what Android said (h, v) and what was sent (pixels right, pixels down).
     */
    private void noteScroll(MotionEvent e, float h, float v, int dx, int dy) {
        String k = "scroll source=0x" + Integer.toHexString(e.getSource()) + " tool=" + e.getToolType(0)
                + " class=" + e.getClassification() + " h" + (int) Math.signum(h) + " v" + (int) Math.signum(v);
        if (seenKinds.add(k)) {
            android.util.Log.i("IstessoAura", k + " AXIS_HSCROLL=" + h + " AXIS_VSCROLL=" + v
                    + " sent right=" + dx + " down=" + dy + " (down > 0: the host scrolls down, as for a wheel turned towards the person)");
        }
    }

    private void buttonsOf(MotionEvent e) {
        int now = e.getButtonState();
        int[][] map = {
            {MotionEvent.BUTTON_PRIMARY, left()},
            {MotionEvent.BUTTON_SECONDARY, 0x111},
            {MotionEvent.BUTTON_TERTIARY, 0x112},
            {MotionEvent.BUTTON_BACK, 0x113},
            {MotionEvent.BUTTON_FORWARD, 0x114},
        };
        for (int[] m : map) {
            boolean was = (buttons & m[0]) != 0;
            boolean is = (now & m[0]) != 0;
            if (was != is) {
                send(Native.INPUT_BUTTON, m[1], is ? 1 : 0);
            }
        }
        buttons = now;
    }

    private static boolean fromMouse(MotionEvent e) {
        return e.isFromSource(InputDevice.SOURCE_MOUSE) || e.getToolType(0) == MotionEvent.TOOL_TYPE_MOUSE;
    }

    private boolean generic(MotionEvent e) {
        noteKind("generic", e);
        switch (e.getActionMasked()) {
            case MotionEvent.ACTION_HOVER_ENTER:
            case MotionEvent.ACTION_HOVER_MOVE:
                place(e);
                return true;
            case MotionEvent.ACTION_BUTTON_PRESS:
            case MotionEvent.ACTION_BUTTON_RELEASE:
                if (place(e)) {
                    buttonsOf(e);
                }
                return true;
            case MotionEvent.ACTION_SCROLL: {
                place(e);
                // The system says how far the content moves; the host wants how far to scroll.
                float h = e.getAxisValue(MotionEvent.AXIS_HSCROLL);
                float v = e.getAxisValue(MotionEvent.AXIS_VSCROLL);
                scrollX += h * WHEEL_PIXELS;
                scrollY -= v * WHEEL_PIXELS;
                int dx = (int) scrollX;
                int dy = (int) scrollY;
                if (dx != 0 || dy != 0) {
                    scrollX -= dx;
                    scrollY -= dy;
                    send(Native.INPUT_SCROLL, dx, dy);
                }
                noteScroll(e, h, v, dx, dy);
                return true;
            }
            default:
                return false;
        }
    }

    private boolean touch(MotionEvent e) {
        view.requestFocus();
        noteKind("touch", e);
        // The puck's touchpad: a two-finger swipe is a scroll; a three-or-more-finger swipe is left to the system.
        int c = e.getClassification();
        if (c == MotionEvent.CLASSIFICATION_TWO_FINGER_SWIPE) {
            scrollX += e.getAxisValue(MotionEvent.AXIS_GESTURE_SCROLL_X_DISTANCE);
            scrollY += e.getAxisValue(MotionEvent.AXIS_GESTURE_SCROLL_Y_DISTANCE);
            int dx = (int) scrollX;
            int dy = (int) scrollY;
            if (dx != 0 || dy != 0) {
                scrollX -= dx;
                scrollY -= dy;
                send(Native.INPUT_SCROLL, dx, dy);
            }
            return true;
        }
        if (c == 4) { // MotionEvent.CLASSIFICATION_MULTI_FINGER_SWIPE (API 34), not in the stubs this build compiles against
            return true;
        }
        if (!place(e)) {
            return true;
        }
        if (fromMouse(e)) {
            buttonsOf(e);
            return true;
        }
        // A finger, a pinch of the hand or the puck's touchpad as touch: the left button.
        switch (e.getActionMasked()) {
            case MotionEvent.ACTION_DOWN:
                if (!touchDown) {
                    touchDown = true;
                    touchButton = left();
                    send(Native.INPUT_BUTTON, touchButton, 1);
                }
                break;
            case MotionEvent.ACTION_UP:
            case MotionEvent.ACTION_CANCEL:
                if (touchDown) {
                    touchDown = false;
                    send(Native.INPUT_BUTTON, touchButton, 0);
                }
                break;
            default:
                break;
        }
        return true;
    }

    /** Fits its child to the picture's shape, centred. */
    static final class Fitted extends FrameLayout {
        private int aspectW;
        private int aspectH;

        Fitted(Context context) {
            super(context);
        }

        void setAspect(int w, int h) {
            aspectW = w;
            aspectH = h;
            requestLayout();
        }

        @Override
        protected void onMeasure(int widthSpec, int heightSpec) {
            int w = MeasureSpec.getSize(widthSpec);
            int h = MeasureSpec.getSize(heightSpec);
            if (aspectW > 0 && aspectH > 0 && w > 0 && h > 0) {
                int cw = w;
                int ch = (int) ((long) w * aspectH / aspectW);
                if (ch > h) {
                    ch = h;
                    cw = (int) ((long) h * aspectW / aspectH);
                }
                for (int i = 0; i < getChildCount(); i++) {
                    View child = getChildAt(i);
                    child.measure(MeasureSpec.makeMeasureSpec(cw, MeasureSpec.EXACTLY),
                            MeasureSpec.makeMeasureSpec(ch, MeasureSpec.EXACTLY));
                }
                setMeasuredDimension(w, h);
                return;
            }
            super.onMeasure(widthSpec, heightSpec);
        }
    }
}
