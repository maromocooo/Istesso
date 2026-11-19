// When a fast core is kept awake (awake.h): the rule, and the thread that follows it, without a
// window. The thread's part keeps one core busy for about a seventh of a second in all.
#include <assert.h>
#include <string.h>
#include <unistd.h>
#include "awake.h"

static const uint64_t second = 1000000000ULL;

static void the_rule(void) {
    const uint64_t t = 500 * second;
    // The whole session: whatever the window and the person do.
    assert(awake_holds(true, false, 0, t));
    assert(awake_holds(true, true, t - 60 * second, t));
    // Otherwise never without input, and never while the window is not the one in front.
    assert(!awake_holds(false, true, 0, t));
    assert(!awake_holds(false, false, t, t));
    assert(!awake_holds(false, false, t - second / 100, t));
    // In front, with input: from the input until AWAKE_AFTER_INPUT_NS after it.
    assert(awake_holds(false, true, t, t));
    assert(awake_holds(false, true, t - second / 2, t));
    assert(awake_holds(false, true, t - AWAKE_AFTER_INPUT_NS + 1, t));
    assert(!awake_holds(false, true, t - AWAKE_AFTER_INPUT_NS, t));
    assert(!awake_holds(false, true, t - 60 * second, t));
    // The next input begins it again.
    assert(awake_holds(false, true, t + 59 * second, t + 60 * second));
    // An input noted just after the clock was read counts as now, not as long ago.
    assert(awake_holds(false, true, t + 1000, t));
    // The pauses inside typing are bridged, and so is the pause between a measurement's keys
    // (0.43 s).
    assert(AWAKE_AFTER_INPUT_NS > 2 * second && AWAKE_AFTER_INPUT_NS <= 5 * second);

    // What began a stretch: an input after the stretch before ended, by its kind; the front when
    // the window came to the front after the latest input; the session for the whole session.
    assert(strcmp(awake_began_by(true, 0, AWAKE_BY_WINDOW, 0, 0), "session") == 0);
    assert(strcmp(awake_began_by(false, t, AWAKE_BY_POINTER, t - second, t - 2 * second), "pointer") == 0);
    assert(strcmp(awake_began_by(false, t, AWAKE_BY_WINDOW, t - second, t - 2 * second), "window") == 0);
    assert(strcmp(awake_began_by(false, t, AWAKE_BY_WAY_IN, t, 0), "way_in") == 0);
    assert(strcmp(awake_began_by(false, t - second, AWAKE_BY_POINTER, t, t - 2 * second), "front") == 0);
    assert(strcmp(awake_began_by(false, 0, AWAKE_BY_WINDOW, 0, 0), "front") == 0);
    // An input while the window was not in front, then the window to the front within the three
    // seconds: the front began it, not the input.
    assert(strcmp(awake_began_by(false, t - second, AWAKE_BY_POINTER, t - 2 * second, t - second / 2), "front") == 0);

    // The kind of a batch: by what the person did when the window knows it, else by the batch.
    assert(awake_by_for(AWAKE_BY_NONE, true) == AWAKE_BY_POINTER);    // the pointer's place alone
    assert(awake_by_for(AWAKE_BY_NONE, false) == AWAKE_BY_WINDOW);    // a key, or a mixed batch
    assert(awake_by_for(AWAKE_BY_WINDOW, true) == AWAKE_BY_WINDOW);   // a click's first batch: its place
    assert(awake_by_for(AWAKE_BY_WINDOW, false) == AWAKE_BY_WINDOW);  // a click's button
    assert(awake_by_for(AWAKE_BY_POINTER, false) == AWAKE_BY_POINTER);// moving, with modifiers coming up
}

static void wait_ms(unsigned ms) {
    usleep(ms * 1000);
}

// The thread from its start, with nothing known about window and input.
static void begin(int form, bool whole_session) {
    awake_form = form;
    awake_whole_session = whole_session;
    atomic_store(&awake_last_input_ns, 0);
    atomic_store(&awake_in_front, false);
    atomic_store(&awake_front_ns, 0);
    atomic_store(&awake_last_by, AWAKE_BY_WINDOW);
    assert(awake_start());
}

static void the_thread(void) {
    // The whole session, spinning: on from the start to the end, in one stretch.
    begin(AWAKE_SPIN, true);
    wait_ms(30);
    awake_finish();
    assert(awake_stretches == 1 && awake_on_ns >= 25 * second / 1000 && awake_turns > 1000 && awake_cpu_ns > 0);

    // By the rule. Input alone does not start it: the window is not the one in front.
    begin(AWAKE_SPIN, false);
    awake_input();
    wait_ms(20);
    awake_finish();
    assert(awake_stretches == 0 && awake_on_ns == 0 && awake_turns == 0);

    // The window in front alone does not start it: nobody types.
    begin(AWAKE_SPIN, false);
    assert(awake_set_in_front(true));
    assert(!awake_set_in_front(true));
    wait_ms(20);
    awake_finish();
    assert(awake_stretches == 0);

    // In front and input: on. The window leaves the front: off. It comes back while the input
    // is still recent: on again.
    begin(AWAKE_SPIN, false);
    awake_set_in_front(true);
    wait_ms(10);
    awake_input();
    wait_ms(20);
    awake_set_in_front(false);
    wait_ms(40);
    awake_set_in_front(true);
    wait_ms(20);
    awake_finish();
    assert(awake_stretches == 2 && awake_on_ns >= 30 * second / 1000);

    // It ends by itself when the latest input is AWAKE_AFTER_INPUT_NS old (here: an input that
    // is 40 ms short of that), and the next input starts it again.
    begin(AWAKE_SPIN, false);
    awake_set_in_front(true);
    atomic_store(&awake_last_input_ns, awake_clock() - AWAKE_AFTER_INPUT_NS + 40 * second / 1000);
    awake_stir();
    wait_ms(120);
    assert(atomic_load(&awake_resting));
    awake_input();
    wait_ms(20);
    awake_finish();
    assert(awake_stretches == 2 && awake_on_ns >= 40 * second / 1000);

    // The cheaper form: wake-ups, never more than a thousand a second, and the processor is
    // left alone between them.
    begin(AWAKE_TICK, true);
    wait_ms(60);
    awake_finish();
    assert(awake_stretches == 1 && awake_turns >= 10 && awake_turns <= awake_on_ns / AWAKE_TICK_NS + 1);
    assert(awake_cpu_ns < awake_on_ns / 2);
}

// The "on":1 line says what began each stretch. The waits are long enough (120 ms) for a loaded
// machine: the thread has to have seen each change before the next.
static void stretches_said(const char *const *want, int count) {
    char line[512];
    int n = 0;
    rewind(awake_log);
    while (fgets(line, sizeof line, awake_log) != NULL) {
        if (strstr(line, "\"kind\":\"awake\"") != NULL && strstr(line, "\"on\":1") != NULL) {
            assert(n < count && strstr(line, want[n]) != NULL);
            n++;
        }
    }
    assert(n == count);
}

static void the_cause(void) {
    const unsigned w = 120;
    awake_log = tmpfile();
    assert(awake_log != NULL);
    begin(AWAKE_SPIN, false);
    awake_set_in_front(true);
    wait_ms(w);
    awake_input_by(AWAKE_BY_POINTER);   // the pointer moved over the window, after a pause
    wait_ms(w);
    // The latest input made old: the stretch ends as after a pause of three seconds.
    atomic_store(&awake_last_input_ns, awake_clock() - AWAKE_AFTER_INPUT_NS - 1);
    wait_ms(w);
    assert(atomic_load(&awake_resting));
    awake_input_by(AWAKE_BY_WINDOW);    // a key
    wait_ms(w);
    awake_set_in_front(false);
    wait_ms(w);
    awake_set_in_front(true);           // back to the front while the key is recent
    wait_ms(w);
    awake_set_in_front(false);
    wait_ms(w);
    awake_input_by(AWAKE_BY_POINTER);   // the pointer moved while the window was not in front
    wait_ms(w);
    assert(atomic_load(&awake_resting));
    awake_set_in_front(true);           // then to the front within three seconds
    wait_ms(w);
    atomic_store(&awake_last_input_ns, awake_clock() - AWAKE_AFTER_INPUT_NS - 1);
    wait_ms(w);
    awake_input_by(AWAKE_BY_WAY_IN);    // the measurement's way in
    wait_ms(w);
    awake_finish();
    assert(awake_stretches == 5);
    const char *spin[] = {"\"by\":\"pointer\"", "\"by\":\"window\"", "\"by\":\"front\"", "\"by\":\"front\"",
                          "\"by\":\"way_in\""};
    stretches_said(spin, 5);
    fclose(awake_log);

    // The tick form says the same.
    awake_log = tmpfile();
    assert(awake_log != NULL);
    begin(AWAKE_TICK, false);
    awake_set_in_front(true);
    wait_ms(w);
    awake_input_by(AWAKE_BY_POINTER);
    wait_ms(w);
    awake_finish();
    const char *tick[] = {"\"by\":\"pointer\""};
    stretches_said(tick, 1);
    fclose(awake_log);

    // The whole session.
    awake_log = tmpfile();
    assert(awake_log != NULL);
    begin(AWAKE_SPIN, true);
    wait_ms(w);
    awake_finish();
    const char *session[] = {"\"by\":\"session\""};
    stretches_said(session, 1);
    fclose(awake_log);
    awake_log = NULL;
}

int main(void) {
    the_rule();
    for (int i = 0; i < 1024; i++) {
        awake_pause();
    }
    // What the thread says goes where the frame log would be.
    awake_log = tmpfile();
    assert(awake_log != NULL);
    the_thread();
    // One line when it starts and one when it has ended, each time; one pair for every stretch.
    char line[512];
    int starts = 0, ends = 0, on = 0, off = 0;
    rewind(awake_log);
    while (fgets(line, sizeof line, awake_log) != NULL) {
        starts += strstr(line, "\"kind\":\"wake_effect\"") != NULL && strstr(line, "\"interactive\":") != NULL;
        ends += strstr(line, "\"kind\":\"wake_effect\"") != NULL && strstr(line, "\"at\":\"end\"") != NULL;
        on += strstr(line, "\"kind\":\"awake\"") != NULL && strstr(line, "\"on\":1") != NULL;
        off += strstr(line, "\"kind\":\"awake\"") != NULL && strstr(line, "\"on\":0") != NULL;
    }
    assert(starts == 6 && ends == 6 && on == 6 && off == 6);
    the_cause();
    printf("awake: the rule holds as written, the thread follows it, and says what began each stretch\n");
    return 0;
}
