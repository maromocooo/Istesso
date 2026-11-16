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

#if defined(__STDC_VERSION__) && __STDC_VERSION__ >= 201112L
_Static_assert(sizeof(IstessoMediaStreamInfo) == 24, "IstessoMediaStreamInfo differs from src/ffi.rs");
_Static_assert(sizeof(IstessoMediaSent) == 24, "IstessoMediaSent differs from src/ffi.rs");
_Static_assert(sizeof(void *) != 8 || sizeof(IstessoMediaFrame) == 72, "IstessoMediaFrame differs from src/ffi.rs");
_Static_assert(sizeof(void *) != 8 || sizeof(IstessoMediaCursor) == 48, "IstessoMediaCursor differs from src/ffi.rs");
#endif

typedef struct IstessoMediaHost IstessoMediaHost;
typedef struct IstessoMediaClient IstessoMediaClient;

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
 * nothing for three seconds, or the pairing was removed); close the client and connect again.
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
