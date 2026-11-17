/*
 * istesso-client: the input's connection of the Mac client, behind a C interface.
 *
 * The application hands over what the person does in its window, in memory, and a thread of
 * this library sends it to the paired host over the control channel (docs/adr/0007-control-
 * channel.md, docs/adr/0018-own-client.md). The thread also looks after the conversation: it
 * notices when it was cut, opens it again, and does not send later what was typed meanwhile.
 *
 * The picture's connection is in the same static library and has its own header,
 * istesso_media.h (istesso_media_watch_*).
 *
 * A function that fails leaves a sentence for istesso_client_last_error(). It never holds a
 * key or an address.
 */
#ifndef ISTESSO_CLIENT_H
#define ISTESSO_CLIENT_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* One thing the person did. Keys and buttons by their Linux numbers (input-event-codes.h). */
typedef struct IstessoInputEvent {
    uint32_t kind; /* 1 key, 2 the pointer's place, 3 button, 4 scrolling */
    int32_t a;     /* key, button: the number. place: x. scrolling: pixels to the right */
    int32_t b;     /* key, button: 1 when it went down. place: y. scrolling: pixels down */
} IstessoInputEvent;

#define ISTESSO_INPUT_KEY 1
#define ISTESSO_INPUT_POINTER_AT 2
#define ISTESSO_INPUT_BUTTON 3
#define ISTESSO_INPUT_SCROLL 4

/* How the conversation stands: istesso_input_state(). */
typedef struct IstessoInputState {
    uint64_t cuts;     /* how often it was cut since it was first open */
    uint32_t open;     /* 1 while there is a conversation */
    uint32_t answered; /* 1 when the host answers "still here", so that silence is noticed */
    uint32_t over;     /* 1 once the host does not take input from here any more */
    uint32_t reserved;
} IstessoInputState;

/* What happened to the conversation: the `event` of IstessoInputTold. */
#define ISTESSO_INPUT_OPEN 1 /* the first conversation is open */
#define ISTESSO_INPUT_CUT 2  /* cut; it is being opened again */
#define ISTESSO_INPUT_BACK 3 /* open again, `after_ms` after the cut */
#define ISTESSO_INPUT_OVER 4 /* refused for good, with `why` in a sentence: the refusal is about
                              * rights or about a request the host does not know; nothing is
                              * tried after it */
#define ISTESSO_INPUT_NOT_YET 5 /* no conversation yet, with `why`: the host could not be reached
                                 * or has nothing that takes input at the moment; it is asked
                                 * again. Told once, before ISTESSO_INPUT_OPEN */

/*
 * Called on the input's thread, never on the caller's. `cuts` is the count so far. `why` is
 * NULL or valid during the call.
 */
typedef void (*IstessoInputTold)(void *context, int32_t event, uint64_t cuts, double after_ms,
                                 const char *why);

/* For `flags`: the input's thread asks to be scheduled as work a person is waiting for. */
#define ISTESSO_INPUT_INTERACTIVE 1u

typedef struct IstessoInput IstessoInput;

/*
 * Starts the input's thread. It opens a conversation with the paired device `peer` (its name,
 * or the beginning of its id) and keeps trying until it has one or is refused; nothing is
 * connected in this call. `config_dir`: where this device's identity and list of paired
 * devices are, NULL for where `istesso` keeps them. `address`: reach the device there only
 * ("192.0.2.1:47801"), NULL for whichever of its remembered addresses answers. `timings`: a
 * file to write to when input was passed on and how often the conversation was cut (times
 * and counts only), NULL for none. NULL when nobody was named or a text cannot be read.
 */
IstessoInput *istesso_input_open(const char *config_dir, const char *peer, const char *address,
                                 const char *timings, uint32_t flags, IstessoInputTold told,
                                 void *context);

/*
 * Hands over what the person did, in order. Never waits and never touches the connection.
 * While there is no conversation the events are gone; they are not sent later.
 */
void istesso_input_send(const IstessoInput *input, const IstessoInputEvent *events, size_t count);

/* How it stands. From any thread. 0, or -1. */
int32_t istesso_input_state(const IstessoInput *input, IstessoInputState *state);

/*
 * Ends the conversation and the thread, and returns when both are over (a tenth of a second
 * at most, or when an attempt to connect has ended). The host lets go of every key and button
 * that was down. `told` is not called after this returns.
 */
void istesso_input_close(IstessoInput *input);

#ifdef ISTESSO_MEASURING
/*
 * The measurement's way in for keys. ONLY IN A MEASURING BUILD: the library has these two
 * functions only when it was built with the feature `measure-keys`, and an application that is
 * handed out is built without this definition and without that feature (ADR 0018).
 *
 * Opens <dir>/measure-keys.sock, a datagram socket as the relay's was: one event in a datagram,
 * as JSON. What arrives goes to the host as what is handed over with istesso_input_send goes.
 * The folder is made for its owner alone (0700); one that is already there is used only if it is
 * a real folder of this user's with exactly those rights, and the socket is 0600. Null when it
 * cannot be opened so (the reason is in istesso_client_last_error()). Closed before `input` is.
 */
typedef struct IstessoMeasureKeys IstessoMeasureKeys;
IstessoMeasureKeys *istesso_measure_keys_open(const IstessoInput *input, const char *dir);
void istesso_measure_keys_close(IstessoMeasureKeys *keys);
#endif

/* Why the last call failed on this thread. Valid until the next call on this thread. */
const char *istesso_client_last_error(void);

#ifdef __cplusplus
}
#endif

#endif
