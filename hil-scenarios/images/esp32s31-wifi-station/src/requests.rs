//! Which requests the image answers by itself in each phase of its one
//! station, and which its station work takes over.

use oer_services_wifi_station::{Join, Joined, Left, Request, StationError};

/// Where the boot's station is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Phase {
    /// No join was made.
    Idle,
    /// The join was taken: its credentials are prepared, then the station
    /// runs and the join waits for its link.
    Joined,
    /// A leave was taken and the station is stopping.
    Leaving,
    /// The station stopped.
    Stopped,
}

/// What the image does with one request.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Action {
    /// Answer it now.
    Answer(Answer),
    /// Answer with the station's status.
    Status,
    /// Take the boot's join.
    Join(Join),
    /// Take the leave.
    Leave,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Answer {
    Joined(Joined),
    Left(Left),
}

/// What the image does with `request` in `phase`: one join per boot, a
/// leave only of a joined station and only once.
pub(crate) fn action(phase: Phase, request: Request) -> Action {
    match (phase, request) {
        (_, Request::GetStatus(_)) => Action::Status,
        (Phase::Idle, Request::Join(join)) => Action::Join(join),
        (_, Request::Join(_)) => {
            Action::Answer(Answer::Joined(Joined::Refused(StationError::AlreadyJoined)))
        }
        (Phase::Idle, Request::Leave(_)) => {
            Action::Answer(Answer::Left(Left::Refused(StationError::NotJoined)))
        }
        (Phase::Joined, Request::Leave(_)) => Action::Leave,
        (Phase::Leaving | Phase::Stopped, Request::Leave(_)) => {
            Action::Answer(Answer::Left(Left::Refused(StationError::Unavailable)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_services_wifi_station::{GetStatus, Leave};

    fn join() -> Join {
        Join {
            ssid: heapless::String::try_from("lab").unwrap(),
            passphrase: heapless::String::try_from("passphrase").unwrap(),
            channel: 6,
        }
    }

    #[test]
    fn every_phase_answers_the_status() {
        for phase in [Phase::Idle, Phase::Joined, Phase::Leaving, Phase::Stopped] {
            assert_eq!(action(phase, Request::GetStatus(GetStatus)), Action::Status);
        }
    }

    #[test]
    fn the_boot_takes_one_join_and_refuses_every_later_one() {
        assert_eq!(
            action(Phase::Idle, Request::Join(join())),
            Action::Join(join())
        );
        for phase in [Phase::Joined, Phase::Leaving, Phase::Stopped] {
            assert_eq!(
                action(phase, Request::Join(join())),
                Action::Answer(Answer::Joined(Joined::Refused(StationError::AlreadyJoined)))
            );
        }
    }

    #[test]
    fn only_a_joined_station_takes_a_leave() {
        assert_eq!(
            action(Phase::Idle, Request::Leave(Leave)),
            Action::Answer(Answer::Left(Left::Refused(StationError::NotJoined)))
        );
        assert_eq!(action(Phase::Joined, Request::Leave(Leave)), Action::Leave);
        for phase in [Phase::Leaving, Phase::Stopped] {
            assert_eq!(
                action(phase, Request::Leave(Leave)),
                Action::Answer(Answer::Left(Left::Refused(StationError::Unavailable)))
            );
        }
    }
}
