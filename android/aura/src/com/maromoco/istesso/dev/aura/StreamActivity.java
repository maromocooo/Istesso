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
 * One panel with the host's picture. Keys and the pointer made over it go to the host. While the
 * panel is in the background the picture is let go (the host gives its encoder back) and the
 * input is closed (the host lets go of whatever was held down).
 */
public final class StreamActivity extends Activity implements SurfaceHolder.Callback, Picture.Told {
    private static final String TAG = "istesso-aura";
    /** One notch of a wheel, in the pixels the host scrolls by (as the Mac client counts it). */
    private static final float WHEEL_PIXELS = 15f;

    private Settings settings;
    private Fitted frame;
    private SurfaceView view;
    private TextView status;
    private String decoderLine = "";
    private String inputLine = "";
    private String pictureLine = "";

    private Picture picture;
    private boolean surfaceReady;
    private boolean resumed;
    private long input;
    private WifiManager.WifiLock wifiLock;

    private volatile int pictureWidth;
    private volatile int pictureHeight;
    private int buttons;
    private boolean touchDown;
    private float scrollX;
    private float scrollY;

    private final Runnable inputWatch = new Runnable() {
        @Override
        public void run() {
            showInput();
            if (resumed) {
                status.postDelayed(this, 1000);
            }
        }
    };

    @Override
    protected void onCreate(Bundle saved) {
        super.onCreate(saved);
        settings = new Settings(this);
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
        root.addView(status, new FrameLayout.LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT,
                ViewGroup.LayoutParams.WRAP_CONTENT, Gravity.TOP | Gravity.START));
        setContentView(root);
        line(getString(R.string.mark));

        WifiManager wifi = (WifiManager) getApplicationContext().getSystemService(Context.WIFI_SERVICE);
        if (wifi != null) {
            wifiLock = wifi.createWifiLock(WifiManager.WIFI_MODE_FULL_LOW_LATENCY, "istesso-aura");
            wifiLock.setReferenceCounted(false);
        }
    }

    @Override
    protected void onResume() {
        super.onResume();
        resumed = true;
        if (wifiLock != null) {
            wifiLock.acquire();
        }
        // Both connections name the paired host; without one, nothing is connected.
        String peer = settings.hostId();
        if (peer.trim().isEmpty()) {
            input = 0;
            inputLine = getString(R.string.need_pair);
        } else {
            input = Native.inputOpen(settings.configDir, peer, Settings.join(settings.host(), settings.inputPort()));
            if (input == 0) {
                String why = Native.lastError(Native.ERROR_PAIRING);
                inputLine = getString(R.string.input_over) + " "
                        + (why.isEmpty() ? Native.lastError(Native.ERROR_INPUT) : why);
            }
        }
        status.post(inputWatch);
        view.requestFocus();
        startPicture();
    }

    @Override
    protected void onPause() {
        resumed = false;
        stopPicture();
        if (input != 0) {
            Native.inputClose(input);
            input = 0;
        }
        buttons = 0;
        touchDown = false;
        if (wifiLock != null && wifiLock.isHeld()) {
            wifiLock.release();
        }
        super.onPause();
    }

    private void startPicture() {
        if (!resumed || !surfaceReady || picture != null) {
            return;
        }
        picture = new Picture(settings.configDir, Settings.join(settings.host(), settings.picturePort()),
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
        runOnUiThread(() -> { pictureLine = getString(R.string.connecting); show(); });
    }

    @Override
    public void connected(int width, int height, int fps, int chroma) {
        pictureWidth = width;
        pictureHeight = height;
        runOnUiThread(() -> {
            frame.setAspect(width, height);
            pictureLine = getString(R.string.connected, width, height, fps, chroma);
            show();
        });
    }

    @Override
    public void cut(long cuts) {
        runOnUiThread(() -> { pictureLine = getString(R.string.cut, (int) cuts); show(); });
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
        runOnUiThread(() -> { pictureLine = text; show(); });
    }

    private void showInput() {
        if (input == 0) {
            show();
            return;
        }
        long[] s = Native.inputState(input);
        if (s != null) {
            inputLine = s[3] != 0 ? getString(R.string.input_over)
                    : s[1] != 0 ? getString(R.string.input_open) : getString(R.string.input_waiting);
        }
        show();
    }

    private void line(String text) {
        status.setText(text);
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
    private void buttonsOf(MotionEvent e) {
        int now = e.getButtonState();
        int[][] map = {
            {MotionEvent.BUTTON_PRIMARY, 0x110},
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
                scrollX += e.getAxisValue(MotionEvent.AXIS_HSCROLL) * WHEEL_PIXELS;
                scrollY -= e.getAxisValue(MotionEvent.AXIS_VSCROLL) * WHEEL_PIXELS;
                int dx = (int) scrollX;
                int dy = (int) scrollY;
                if (dx != 0 || dy != 0) {
                    scrollX -= dx;
                    scrollY -= dy;
                    send(Native.INPUT_SCROLL, dx, dy);
                }
                return true;
            }
            default:
                return false;
        }
    }

    private boolean touch(MotionEvent e) {
        view.requestFocus();
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
                    send(Native.INPUT_BUTTON, 0x110, 1);
                }
                break;
            case MotionEvent.ACTION_UP:
            case MotionEvent.ACTION_CANCEL:
                if (touchDown) {
                    touchDown = false;
                    send(Native.INPUT_BUTTON, 0x110, 0);
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
