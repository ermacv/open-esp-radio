/*
 * Host test of the station link's state machine (../main/link.c), which
 * holds no SDK call. It needs a C compiler, so it is not part of the cargo
 * tests:
 *
 *     cc -std=c11 -Wall -Wextra -Werror -I../main link_test.c ../main/link.c -o <scratch>/link_test
 *     <scratch>/link_test
 *
 * Exit 0 when every case holds.
 */

#include <stdio.h>
#include <stdlib.h>

#include "link.h"

static int s_failures;

#define CHECK(condition)                                                                  \
    do {                                                                                  \
        if (!(condition)) {                                                               \
            fprintf(stderr, "%s:%d: %s\n", __FILE__, __LINE__, #condition);               \
            s_failures++;                                                                 \
        }                                                                                 \
    } while (0)

/* A scripted owner: the events pending at each drain, the actions taken and
 * the facts' read. */
typedef struct {
    const link_event_t *events;
    size_t count;
    size_t next;
    /* Events become pending only up to here; `verify` releases the rest. */
    size_t pending;
    int reconnects;
    int reported;
    bool reconnect_works;
    bool read_works;
    int reads;
} script_t;

static bool next(void *context, link_event_t *event)
{
    script_t *script = context;
    if (script->next >= script->pending) {
        return false;
    }
    *event = script->events[script->next++];
    return true;
}

static bool act(void *context, link_action_t action)
{
    script_t *script = context;
    if (action == LINK_ACT_RECONNECT) {
        script->reconnects++;
        return script->reconnect_works;
    }
    if (action == LINK_ACT_REPORT_LOST) {
        script->reported++;
    }
    return true;
}

/* Reading the facts takes time: every scripted event arrives meanwhile. */
static bool verify(void *context)
{
    script_t *script = context;
    script->reads++;
    script->pending = script->count;
    return script->read_works;
}

static const link_event_t CONNECTED = { LINK_EVENT_CONNECTED, 0 };
static const link_event_t GOT_IP = { LINK_EVENT_GOT_IP, 0 };
static const link_event_t DISCONNECTED = { LINK_EVENT_DISCONNECTED, 15 };
static const link_event_t LOST_IP = { LINK_EVENT_LOST_IP, 0 };
static const link_event_t OVERFLOW = { LINK_EVENT_OVERFLOW, 0 };

static link_t joining(void)
{
    link_t link;
    link_init(&link);
    CHECK(link_begin_join(&link));
    CHECK(!link_begin_join(&link));
    return link;
}

/* Apply `events` in order, carrying out reconnects. */
static void apply_all(link_t *link, const link_event_t *events, size_t count)
{
    for (size_t index = 0; index < count; index++) {
        link_apply(link, &events[index]);
    }
}

static void a_join_with_an_address_goes_up_once_the_facts_are_read(void)
{
    link_t link = joining();
    const link_event_t events[] = { CONNECTED, GOT_IP };
    apply_all(&link, events, 2);
    CHECK(link_candidate(&link));
    script_t script = { .read_works = true, .reconnect_works = true };
    CHECK(link_confirm(&link, next, act, verify, &script) == LINK_CONFIRM_UP);
    CHECK(link.state == LINK_UP && script.reads == 1);
}

static void a_disconnection_queued_behind_the_address_is_no_join(void)
{
    /* GOT_IP was taken; DISCONNECTED was already queued behind it. */
    link_t link = joining();
    const link_event_t taken[] = { CONNECTED, GOT_IP };
    apply_all(&link, taken, 2);
    const link_event_t queued[] = { DISCONNECTED };
    script_t script = { .events = queued, .count = 1, .pending = 1, .read_works = true,
                        .reconnect_works = true };
    CHECK(link_confirm(&link, next, act, verify, &script) == LINK_CONFIRM_NOT_YET);
    CHECK(link.state == LINK_JOINING && script.reconnects == 1 && script.reads == 0);
    CHECK(link.disconnects == 1 && link.last_reason == 15);
}

static void a_disconnection_while_the_facts_are_read_is_no_join(void)
{
    link_t link = joining();
    const link_event_t taken[] = { CONNECTED, GOT_IP };
    apply_all(&link, taken, 2);
    const link_event_t during[] = { DISCONNECTED };
    script_t script = { .events = during, .count = 1, .pending = 0, .read_works = false,
                        .reconnect_works = true };
    /* The read failed because the link went: not a failed join yet. */
    CHECK(link_confirm(&link, next, act, verify, &script) == LINK_CONFIRM_NOT_YET);
    CHECK(link.state == LINK_JOINING && script.reconnects == 1);
}

static void a_reconnection_while_the_facts_are_read_reads_them_again(void)
{
    /* The read began on the first link; it ended and a second came up
     * before the drain: both flags are set again, but the facts are the
     * first link's. */
    link_t link = joining();
    const link_event_t taken[] = { CONNECTED, GOT_IP };
    apply_all(&link, taken, 2);
    const link_event_t during[] = { DISCONNECTED, CONNECTED, GOT_IP };
    script_t script = { .events = during, .count = 3, .pending = 0, .read_works = true,
                        .reconnect_works = true };
    CHECK(link_confirm(&link, next, act, verify, &script) == LINK_CONFIRM_NOT_YET);
    CHECK(link.state == LINK_JOINING && link_candidate(&link) && script.reads == 1);
    /* The next confirmation reads the second link's facts. */
    CHECK(link_confirm(&link, next, act, verify, &script) == LINK_CONFIRM_UP);
    CHECK(link.state == LINK_UP && script.reads == 2);
}

static void a_repeated_address_during_the_read_reads_the_facts_again(void)
{
    link_t link = joining();
    const link_event_t taken[] = { CONNECTED, GOT_IP };
    apply_all(&link, taken, 2);
    const link_event_t during[] = { GOT_IP };
    script_t script = { .events = during, .count = 1, .pending = 0, .read_works = true,
                        .reconnect_works = true };
    CHECK(link_confirm(&link, next, act, verify, &script) == LINK_CONFIRM_NOT_YET);
    CHECK(link_confirm(&link, next, act, verify, &script) == LINK_CONFIRM_UP);
    CHECK(script.reads == 2);
}

static void a_failed_read_of_a_standing_link_fails_the_join(void)
{
    link_t link = joining();
    const link_event_t taken[] = { CONNECTED, GOT_IP };
    apply_all(&link, taken, 2);
    script_t script = { .read_works = false, .reconnect_works = true };
    CHECK(link_confirm(&link, next, act, verify, &script) == LINK_CONFIRM_FAILED);
    CHECK(link.state == LINK_DOWN);
    CHECK(!link_begin_join(&link));
}

static void a_refused_reconnect_fails_the_join(void)
{
    link_t link = joining();
    const link_event_t taken[] = { CONNECTED, GOT_IP };
    apply_all(&link, taken, 2);
    const link_event_t queued[] = { DISCONNECTED };
    script_t script = { .events = queued, .count = 1, .pending = 1, .read_works = true,
                        .reconnect_works = false };
    CHECK(link_confirm(&link, next, act, verify, &script) == LINK_CONFIRM_FAILED);
    CHECK(link.state == LINK_DOWN);
}

static void an_address_of_an_ended_association_does_not_count(void)
{
    link_t link = joining();
    const link_event_t events[] = { CONNECTED, DISCONNECTED, GOT_IP };
    apply_all(&link, events, 3);
    CHECK(!link_candidate(&link));
    const link_event_t again[] = { CONNECTED };
    apply_all(&link, again, 1);
    CHECK(!link_candidate(&link));
    apply_all(&link, &GOT_IP, 1);
    CHECK(link_candidate(&link));
}

static void a_joined_link_that_ends_is_reported_and_stays_down(void)
{
    for (int kind = 0; kind < 2; kind++) {
        link_t link = joining();
        const link_event_t events[] = { CONNECTED, GOT_IP };
        apply_all(&link, events, 2);
        script_t script = { .read_works = true, .reconnect_works = true };
        CHECK(link_confirm(&link, next, act, verify, &script) == LINK_CONFIRM_UP);
        const link_event_t *end = kind == 0 ? &DISCONNECTED : &LOST_IP;
        CHECK(link_apply(&link, end) == LINK_ACT_REPORT_LOST);
        CHECK(link.state == LINK_DOWN);
        /* No reconnect after the join: a new association does not raise it. */
        CHECK(link_apply(&link, &CONNECTED) == LINK_ACT_NONE);
        CHECK(link_apply(&link, &GOT_IP) == LINK_ACT_NONE);
        CHECK(link.state == LINK_DOWN && !link_candidate(&link));
    }
}

static void lost_events_leave_the_state_unknown_for_good(void)
{
    link_t link = joining();
    const link_event_t events[] = { CONNECTED, GOT_IP };
    apply_all(&link, events, 2);
    const link_event_t queued[] = { OVERFLOW };
    script_t script = { .events = queued, .count = 1, .pending = 1, .read_works = true,
                        .reconnect_works = true };
    CHECK(link_confirm(&link, next, act, verify, &script) == LINK_CONFIRM_FAULTED);
    CHECK(link.state == LINK_FAULTED);
    apply_all(&link, events, 2);
    CHECK(link.state == LINK_FAULTED && !link_candidate(&link));

    link_t up = joining();
    apply_all(&up, events, 2);
    script_t clean = { .read_works = true, .reconnect_works = true };
    CHECK(link_confirm(&up, next, act, verify, &clean) == LINK_CONFIRM_UP);
    CHECK(link_apply(&up, &OVERFLOW) == LINK_ACT_REPORT_LOST);
    CHECK(up.state == LINK_FAULTED);
}

static void a_candidate_needs_a_join(void)
{
    link_t link;
    link_init(&link);
    const link_event_t events[] = { CONNECTED, GOT_IP };
    apply_all(&link, events, 2);
    CHECK(!link_candidate(&link) && link.state == LINK_IDLE);
    script_t script = { .read_works = true, .reconnect_works = true };
    CHECK(link_confirm(&link, next, act, verify, &script) == LINK_CONFIRM_NOT_YET);
    CHECK(link.state == LINK_IDLE);
}

int main(void)
{
    a_join_with_an_address_goes_up_once_the_facts_are_read();
    a_disconnection_queued_behind_the_address_is_no_join();
    a_disconnection_while_the_facts_are_read_is_no_join();
    a_reconnection_while_the_facts_are_read_reads_them_again();
    a_repeated_address_during_the_read_reads_the_facts_again();
    a_failed_read_of_a_standing_link_fails_the_join();
    a_refused_reconnect_fails_the_join();
    an_address_of_an_ended_association_does_not_count();
    a_joined_link_that_ends_is_reported_and_stays_down();
    lost_events_leave_the_state_unknown_for_good();
    a_candidate_needs_a_join();
    if (s_failures != 0) {
        fprintf(stderr, "%d checks failed\n", s_failures);
        return EXIT_FAILURE;
    }
    printf("link_test: every case holds\n");
    return EXIT_SUCCESS;
}
