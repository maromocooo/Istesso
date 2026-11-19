/*
 * istesso-media: carries the picture of an application from a host to a paired device,
 * encrypted and authenticated (docs/adr/0015-media-transport.md).
 *
 * The C interface of the Rust crate `istesso-media`, written by hand; it has to say what
 * src/ffi.rs says. Link the static library (libistesso_media.a) or load the shared one
 * (libistesso_media.so, libistesso_media.dylib).
 *
 * Both ends use the identity of the device they run on and its list of paired devices, as
 * `istesso` keeps them. A host only sends to a device that is paired and may look at it; a
 * client only takes a picture from a device it is paired with. Nothing here creates an
 * identity or changes a pairing.
 *
 * A function that fails leaves a sentence for istesso_media_last_error(). It never holds a
 * key or an address.
 */
#ifndef ISTESSO_MEDIA_H
#define ISTESSO_MEDIA_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* The port a host listens on unless told otherwise. */
#define ISTESSO_MEDIA_DEFAULT_PORT 47803

/* What a host tells its viewers about the picture. */
typedef struct IstessoMediaStreamInfo {
    uint32_t codec;  /* 1: H.265, one access unit per frame, NAL units behind start codes
                        (Annex B), key frames with their parameter sets. 0: not known */
    uint32_t width;  /* size of the picture in its own pixels */
    uint32_t height;
    uint32_t fps;    /* most frames in any one second */
    uint32_t chroma; /* 420 or 444. 0: not known */
    uint32_t colour; /* 1: sRGB, to be shown like the viewer's own windows. 0: nothing is said */
} IstessoMediaStreamInfo;

/* What became of a frame handed to istesso_media_host_send(). */
typedef struct IstessoMediaSent {
    uint64_t seq;      /* the frame's sequence number */
    uint32_t viewers;  /* devices it went to; one that waits for a key frame gets nothing before it */
    uint32_t queued;   /* of those, the ones whose connection did not take all of it at once: the
                          rest is on its way and is finished, or given up, within 0.2 s */
    uint32_t messages; /* messages of the encrypted channel it took, for each of them */
    uint32_t reserved;
} IstessoMediaSent;

/* One frame, as istesso_media_client_next() hands it over. */
typedef struct IstessoMediaFrame {
    const uint8_t *data;    /* the encoded frame; valid until the next call with the same client */
    size_t len;
    uint64_t seq;
    uint64_t pts_ns;        /* presentation time stamp, on the clock of the host's pipeline */
    uint64_t encoded_ns;    /* when the host's encoder had finished the frame, on the host's clock */
    uint64_t first_ns;      /* when the frame's first bytes were there to be read, on istesso_media_now_ns() */
    uint64_t whole_ns;      /* when the whole frame had been read and opened, on the same clock */
    uint32_t key_frame;     /* 1 for a key frame */
    uint32_t discontinuity; /* 1 when the host sent this client nothing between the frame before and this one */
    uint32_t messages;      /* messages of the encrypted channel the frame came in */
    uint32_t reserved;
} IstessoMediaFrame;

/*
 * What the pointer's cursor looks like on the host. The host does not draw it into the frames
 * then; the client draws it where its own pointer is. The image comes from whatever application
 * runs on the host: neither end takes one whose sides are not between 1 and 256 pixels, whose
 * hot spot is not inside it, or whose bytes are not as many as its size says.
 */
typedef struct IstessoMediaCursor {
    uint32_t shape;       /* 0: the application has hidden the pointer, nothing is drawn.
                             1: a cursor there is no image of; the client shows one of its own.
                             2: an image */
    uint32_t width;       /* size of the image in pixels (shape 2) */
    uint32_t height;
    uint32_t hotspot_x;   /* the pixel of the image that lies on the pointer's position */
    uint32_t hotspot_y;
    uint32_t scale;       /* pixels of the image to one pixel of the picture */
    const uint8_t *pixels; /* four bytes a pixel: red, green, blue, alpha, the colours multiplied
                              by alpha; rows from the top. NULL unless shape is 2 */
    size_t pixels_len;
    const char *name;     /* the cursor's name when shape is 1; may be NULL */
} IstessoMediaCursor;

/*
 * Trial only (ADR 0015, "文字の入力の欄"): the text input field of the application with the
 * keyboard, and where its text cursor is, in the picture's pixels. No typed text is in it.
 */
typedef struct IstessoMediaTextInput {
    uint32_t state;   /* 0: nothing known. 1: a field. 2: a password field. 3: not in a field */
    int32_t x;        /* the text cursor's rectangle, from the picture's top left */
    int32_t y;
    uint32_t width;   /* 0 to 65535 */
    uint32_t height;
    uint32_t serial;  /* 0 to 255: changes whenever another field, or the same one enabled again,
                         has the keyboard; the client ends its composition then */
    uint32_t dropped; /* 0 to 65535: pieces of committed text the host had nowhere to put so far;
                         when it grows, the client keeps what it sent last for the person */
} IstessoMediaTextInput;

#if defined(__STDC_VERSION__) && __STDC_VERSION__ >= 201112L
_Static_assert(sizeof(IstessoMediaTextInput) == 28, "IstessoMediaTextInput differs from src/ffi.rs");
_Static_assert(sizeof(IstessoMediaStreamInfo) == 24, "IstessoMediaStreamInfo differs from src/ffi.rs");
_Static_assert(sizeof(IstessoMediaSent) == 24, "IstessoMediaSent differs from src/ffi.rs");
_Static_assert(sizeof(void *) != 8 || sizeof(IstessoMediaFrame) == 72, "IstessoMediaFrame differs from src/ffi.rs");
_Static_assert(sizeof(void *) != 8 || sizeof(IstessoMediaCursor) == 48, "IstessoMediaCursor differs from src/ffi.rs");
#endif

typedef struct IstessoMediaHost IstessoMediaHost;
typedef struct IstessoMediaClient IstessoMediaClient;
typedef struct IstessoMediaWatch IstessoMediaWatch;

/* How a lasting viewer stands: istesso_media_watch_state(). */
typedef struct IstessoMediaWatchState {
    uint64_t cuts;      /* how often the connection was cut and taken up again, or is being */
    uint64_t silent_ms; /* at the last "connected": for how long the host had not been heard */
    uint32_t again;     /* at the last "connected": 1 when it was a connection again after a cut */
    int32_t why;        /* once it is over, one of ISTESSO_MEDIA_OVER_*; 0 before */
    uint32_t failure;   /* while there is no connection: why it was cut or why the latest attempt
                         * failed, one of ISTESSO_MEDIA_FAILURE_*; 0 otherwise. Up to date after a
                         * call of istesso_media_watch_next that brought no frame */
    uint32_t reserved;
} IstessoMediaWatchState;

/* Why an attempt to reach the host failed, in a kind that names no address. */
#define ISTESSO_MEDIA_FAILURE_REFUSED 1     /* the machine answered; nothing listens there */
#define ISTESSO_MEDIA_FAILURE_UNREACHABLE 2 /* no way to the machine from here (no route, network
                                             * down, or this machine does not let the program out) */
#define ISTESSO_MEDIA_FAILURE_TIMED_OUT 3   /* connecting or the first exchange got no answer in time */
#define ISTESSO_MEDIA_FAILURE_NO_ANSWER 4   /* reached and greeted, then silence or the end */
#define ISTESSO_MEDIA_FAILURE_NOT_READY 5   /* the host said that it cannot do it right now */
#define ISTESSO_MEDIA_FAILURE_OTHER 6

/* Why a lasting viewer is over and does not connect again. */
#define ISTESSO_MEDIA_OVER_ENDED 1       /* the host said that the stream is over */
#define ISTESSO_MEDIA_OVER_NOT_SHOWN 2   /* the host shows the picture to another device, or to none */
#define ISTESSO_MEDIA_OVER_SET_ASIDE 3   /* the host has put the picture aside for the time being */
#define ISTESSO_MEDIA_OVER_NOT_PAIRED 4  /* the two devices are not paired any more */
#define ISTESSO_MEDIA_OVER_FORBIDDEN 5   /* this device may not look at the host */
#define ISTESSO_MEDIA_OVER_UNSUPPORTED 6 /* the host sends something this version cannot show */
#define ISTESSO_MEDIA_OVER_OTHER 7       /* refused otherwise; istesso_media_last_error() says how */

/* ---- Host: the device that shows its picture ---- */

/*
 * Starts listening for paired devices on `bind` ("0.0.0.0:47803", "[::]:47803", an address of
 * one adapter). `config_dir` is where this device's identity and list of paired devices are;
 * NULL for where `istesso` keeps them. `info` is what viewers are told about the picture.
 * NULL on failure.
 */
IstessoMediaHost *istesso_media_host_listen(const char *config_dir, const char *bind,
                                            const IstessoMediaStreamInfo *info);

/*
 * The same, for the picture of a streaming session: nobody is shown it until
 * istesso_media_host_admit() names a device.
 */
IstessoMediaHost *istesso_media_host_listen_closed(const char *config_dir, const char *bind,
                                                   const IstessoMediaStreamInfo *info);

/*
 * Says which one paired device is shown the picture from now on: the one with this id and
 * this public key, both as the list of paired devices has them (`istesso peers list --json`).
 * Both NULL: nobody. Any other viewer is cut off before this returns, and whoever else
 * connects is refused. Being paired and allowed to look is still checked underneath, at every
 * connection and while the stream runs. 0, or -1 when only one of the two was given or a text
 * could not be read; then nobody is admitted.
 */
int32_t istesso_media_host_admit(const IstessoMediaHost *host, const char *device_id,
                                 const char *public_key);

/* Back to every paired device that may look and connects: what a host does that was never told. */
int32_t istesso_media_host_admit_paired(const IstessoMediaHost *host);

/* The port the host listens on (of use after asking for port 0). */
uint16_t istesso_media_host_port(const IstessoMediaHost *host);

/*
 * Sends one encoded frame to every viewer that can decode it. Never waits for a viewer: the
 * frame is put on each connection as far as the connection takes it at once, which on a local
 * network is all of it. The rest is finished behind the caller's back, and a viewer that has
 * not taken a whole frame within 0.2 s is cut off; until then it is sent nothing more.
 * `pts_ns` and `encoded_ns` travel with the frame. `sent` may be NULL. 0, or -1 on failure
 * (a frame of more than 8 MiB). With nobody watching it does nothing, and that is no failure.
 */
int32_t istesso_media_host_send(const IstessoMediaHost *host, const uint8_t *data, size_t len,
                                uint64_t pts_ns, uint64_t encoded_ns, int32_t key_frame,
                                IstessoMediaSent *sent);

/*
 * 1 when a key frame is needed: a viewer has connected, has asked for one, or was held back.
 * 1 once for each time; the caller tells the encoder. Call it regularly, also while no frame
 * is produced.
 */
int32_t istesso_media_host_key_frame_wanted(const IstessoMediaHost *host);

/*
 * Says what the pointer's cursor looks like from now on. A viewer is told the latest cursor
 * only, at most once in a frame's interval, and behind any frame that is on its way to it; a
 * viewer that connects later is told what the cursor is then. 0, or -1 when the cursor is one
 * that may not travel (see IstessoMediaCursor); it then reaches nobody.
 */
int32_t istesso_media_host_set_cursor(const IstessoMediaHost *host, const IstessoMediaCursor *cursor);

/*
 * Trial only: says what the text input field of the application with the keyboard is, as
 * istesso_media_host_set_cursor() says what the cursor is (the latest only, behind frames, to
 * viewers that may be sent the frames, and to one that connects later). Nothing is sent until
 * this is first called. 0, or -1 for a state above 3, a side above 65535, a serial above 255 or
 * a count of dropped text above 65535.
 */
int32_t istesso_media_host_set_text_input(const IstessoMediaHost *host, const IstessoMediaTextInput *field);

/*
 * Offers the picture to paired devices (1, as from the start) or stops offering it (0): every
 * viewer is cut off at once and no caller is taken, or even answered, until it is offered again.
 * For a session that is shown to nobody for the time being.
 */
void istesso_media_host_offer(const IstessoMediaHost *host, int32_t offered);

/* Devices that are being shown the picture now. */
uint32_t istesso_media_host_viewers(const IstessoMediaHost *host);

/* Ends every stream and stops listening. No other thread may be using `host`. */
void istesso_media_host_close(IstessoMediaHost *host);

/* ---- Client: the device that is shown the picture ---- */

/*
 * Connects to the host at `addr` ("192.0.2.1:47803", "[fd00::1]:47803") and asks for its
 * picture. `peer` names the paired device that has to answer there, by name or id; NULL takes
 * any paired device. Connecting, the handshake and the host's answer have `within_ms`
 * altogether. NULL on failure.
 */
IstessoMediaClient *istesso_media_client_connect(const char *config_dir, const char *addr,
                                                 const char *peer, uint32_t within_ms);

/* What the host said about its picture. 0, or -1. */
int32_t istesso_media_client_info(const IstessoMediaClient *client, IstessoMediaStreamInfo *info);

/*
 * Waits for the next frame, for `wait_ms` at most. 1: `frame` is filled in. 2: the host has
 * said what the cursor is; fetch it with istesso_media_client_cursor(). 0: nothing came in
 * that time (the picture stands still). -1: the connection is over (the host ended it, said
 * nothing for a second, or the pairing was removed); close the client. A program that wants
 * the picture back after a cut uses istesso_media_watch_open() instead.
 * One thread calls this.
 */
int32_t istesso_media_client_next(const IstessoMediaClient *client, uint32_t wait_ms,
                                  IstessoMediaFrame *frame);

/*
 * What the host last said the cursor is. 1: `cursor` is filled in; what its pointers point to
 * stays valid until the next call with the same client. 0: the host has not said (a host that
 * draws the cursor into its frames never does). -1: no client. One thread calls this, the one
 * that calls istesso_media_client_next().
 */
int32_t istesso_media_client_cursor(const IstessoMediaClient *client, IstessoMediaCursor *cursor);

/*
 * The picture cannot be decoded from here on: have the host send a key frame. From any
 * thread; the request leaves at once when a thread is waiting in istesso_media_client_next(),
 * and with its next call otherwise.
 */
void istesso_media_client_ask_key_frame(const IstessoMediaClient *client);

/* Closes the connection. No other thread may be using `client`. */
void istesso_media_client_close(IstessoMediaClient *client);

/* ---- A viewer that lasts: it connects again when its connection is cut ---- */

/*
 * A connection can end while both ends are alive (a network may stop delivering on it without
 * a word). This viewer then connects again by itself: at once, then four times a second, and
 * once a second when the host has been away for ten seconds. It does so only when nobody said
 * why the connection ended. When the host said that the stream is over, that the picture is
 * for another device or put aside, or refused this device, it is over, and stays so.
 *
 * `addr` and `peer` as for istesso_media_client_connect(). Nothing is connected here: the
 * first call of istesso_media_watch_next() does that. Later connections are made only to the
 * device the first one reached. NULL when no address was given or it cannot be read.
 */
IstessoMediaWatch *istesso_media_watch_open(const char *config_dir, const char *addr,
                                            const char *peer);

/* Daily windows also follow a clean pipeline restart. Call once, before sharing the new watch.
 * Detach, reassignment and refusal remain terminal. */
void istesso_media_watch_follow_restarts(IstessoMediaWatch *watch);

/*
 * Waits for the next thing, for `wait_ms` at most (an attempt to connect may take longer:
 * three seconds the first time, one second after a cut).
 *    1: `frame` is filled in.
 *    2: the host has said what the cursor is; fetch it with istesso_media_watch_cursor().
 *    0: nothing came in that time.
 *    3: the connection was cut (it failed, or the host said nothing for a second) and is being
 *       made again. What is on the screen is not the host's picture of this moment any more.
 *    4: connected, the first time or again; istesso_media_watch_info() says what the host
 *       sends now, and the first frame that follows is a key frame.
 *    5: (trial only) the host has said what the text input field is; fetch it with
 *       istesso_media_watch_text_input().
 *   -1: it is over and is not tried again; istesso_media_watch_state() says why, and
 *       istesso_media_last_error() in a sentence.
 * One thread calls this.
 */
int32_t istesso_media_watch_next(const IstessoMediaWatch *watch, uint32_t wait_ms,
                                 IstessoMediaFrame *frame);

/* What the host said about its picture on the connection there is. 0, or -1 without one. */
int32_t istesso_media_watch_info(const IstessoMediaWatch *watch, IstessoMediaStreamInfo *info);

/* As istesso_media_client_cursor(), on the connection there is. */
int32_t istesso_media_watch_cursor(const IstessoMediaWatch *watch, IstessoMediaCursor *cursor);

/* Trial only: what the host last said the text input field is. 1: `field` is filled in. 0: it
 * has said nothing yet. -1: no connection. The thread that calls istesso_media_watch_next(). */
int32_t istesso_media_watch_text_input(const IstessoMediaWatch *watch, IstessoMediaTextInput *field);

/* How it stands. From any thread. 0, or -1. */
int32_t istesso_media_watch_state(const IstessoMediaWatch *watch, IstessoMediaWatchState *state);

/* As istesso_media_client_ask_key_frame(). From any thread. */
void istesso_media_watch_ask_key_frame(const IstessoMediaWatch *watch);

/* Closes the connection, if there is one. No other thread may be using `watch`. */
void istesso_media_watch_close(IstessoMediaWatch *watch);

/* ---- For both ---- */

/*
 * The clock `first_ns` and `whole_ns` are read from, in nanoseconds: on macOS the uptime clock
 * (clock_gettime_nsec_np(CLOCK_UPTIME_RAW)), elsewhere the monotonic clock.
 */
uint64_t istesso_media_now_ns(void);

/*
 * Asks the system to schedule the calling thread as one that has work every `period_ns`, for
 * `computation_ns` at most each time, to be done within `constraint_ns` of when it could have
 * started (macOS: a time-constraint thread, as the threads of the sound system are). 1 when
 * the system agreed, 0 otherwise and on other systems. Not a privileged operation.
 */
int32_t istesso_media_thread_real_time(uint64_t period_ns, uint64_t computation_ns,
                                       uint64_t constraint_ns);

/* Why the last call on this thread failed. Valid until the next call on this thread. */
const char *istesso_media_last_error(void);

#ifdef __cplusplus
}
#endif

#endif /* ISTESSO_MEDIA_H */
