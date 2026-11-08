/*
 * The station link's state, owned by one context: the console task. The
 * Wi-Fi and IP event handlers only post link events to it, in the order the
 * event loop delivers them; this state machine applies them. It holds no
 * SDK call, so the host test (../test/link_test.c) runs it.
 */
#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

typedef enum {
    /* No join was asked for. */
    LINK_IDLE,
    /* A join waits for association and an address. */
    LINK_JOINING,
    /* Joined: associated with an address, and nothing since took either. */
    LINK_UP,
    /* A join failed, or the link of a join ended. Final for this boot. */
    LINK_DOWN,
    /* Link events were lost: the state is unknown. Final for this boot. */
    LINK_FAULTED,
} link_state_t;

typedef enum {
    LINK_EVENT_CONNECTED,
    LINK_EVENT_DISCONNECTED,
    LINK_EVENT_GOT_IP,
    LINK_EVENT_LOST_IP,
    /* The handlers could not post an event: some event is missing. */
    LINK_EVENT_OVERFLOW,
} link_event_kind_t;

typedef struct {
    link_event_kind_t kind;
    /* The disconnection's reason code; 0 otherwise. */
    int32_t reason;
} link_event_t;

/* What the owner does after an event. */
typedef enum {
    LINK_ACT_NONE,
    /* A disconnection during the join: connect again. */
    LINK_ACT_RECONNECT,
    /* The joined link ended (or became unknown): report it. */
    LINK_ACT_REPORT_LOST,
} link_action_t;

typedef struct {
    link_state_t state;
    bool associated;
    bool addressed;
    uint32_t disconnects;
    int32_t last_reason;
} link_t;

void link_init(link_t *link);

/* IDLE → JOINING; false in any other state. */
bool link_begin_join(link_t *link);

link_action_t link_apply(link_t *link, const link_event_t *event);

/* Whether a join is associated and addressed, as far as the events applied
 * so far say. */
bool link_candidate(const link_t *link);

/* JOINING → DOWN: the join failed. */
void link_fail(link_t *link);

/* The next pending event, without waiting; false when none is pending. */
typedef bool (*link_next_fn)(void *context, link_event_t *event);
/* Carry out `action`; false when it failed (a reconnect the driver refused). */
typedef bool (*link_act_fn)(void *context, link_action_t action);
/* Read the joined link's facts (access point, mode, address); false when a
 * read failed. */
typedef bool (*link_verify_fn)(void *context);

/* Apply every pending event and carry out its action. False when an action
 * failed: a join then fails. */
bool link_drain(link_t *link, link_next_fn next, link_act_fn act, void *context);

typedef enum {
    /* Every event so far says joined, and the facts were read: UP. */
    LINK_CONFIRM_UP,
    /* The pending events took the association or the address: keep
     * waiting for the join. */
    LINK_CONFIRM_NOT_YET,
    /* Still joined by every event, but the facts could not be read, or a
     * reconnect failed: the join failed (DOWN). */
    LINK_CONFIRM_FAILED,
    /* Link events were lost (FAULTED). */
    LINK_CONFIRM_FAULTED,
} link_confirm_t;

/* Confirm a join candidate: drain the pending events, read the facts,
 * drain again, and go UP only when every event up to the last drain still
 * says joined. */
link_confirm_t link_confirm(link_t *link, link_next_fn next, link_act_fn act,
                            link_verify_fn verify, void *context);
