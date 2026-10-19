//! Asynchronous key-data unwrap around the sans-IO station supplicant.

use oer_ieee80211_rsn::{
    OwnedEapolFrame, Pmk,
    aes::AsyncRsnKeyUnwrap,
    frames::RsnTxFrame,
    supplicant::{
        RsnConnectedAction, RsnConnectedProcessError, RsnConnectedSupplicant,
        RsnGroupKeyInstallRequest, RsnStaProcessError, RsnStaReady, RsnStaSupplicant,
        RsnStaSupplicantAction,
    },
};

/// Consume one peer EAPOL-Key frame and resolve a requested key-data unwrap
/// through `unwrap`.
///
/// The resolved [`RsnStaReady`] type cannot carry an unfinished unwrap.
/// An encrypted Message 3 yields initial or group-only install work, and an unwrap failure is
/// [`RsnStaProcessError::KeyUnwrap`] after the supplicant has rejected the key
/// data.
pub async fn process_frame<const N: usize, U: AsyncRsnKeyUnwrap>(
    supplicant: &mut RsnStaSupplicant,
    frame: OwnedEapolFrame<N>,
    pmk: &Pmk,
    unwrap: &mut U,
) -> Result<RsnStaReady<N>, RsnStaProcessError<U::Error>> {
    match supplicant.on_frame(frame, pmk)? {
        RsnStaSupplicantAction::UnwrapKeyData(request) => {
            let unwrapped = unwrap
                .unwrap_key_data(request.kek(), request.wrapped_key_data())
                .await;
            supplicant.complete_key_data_unwrap(request, unwrapped)
        }
        RsnStaSupplicantAction::Ready(action) => Ok(action),
    }
}

/// What one Group Message 1 asks of a connected station.
pub enum RsnGroupMessage1Step<const N: usize> {
    /// A new group key: install it, then complete the request with
    /// [`RsnConnectedSupplicant::complete_group_key_install`], which yields
    /// Group Message 2.
    Install(RsnGroupKeyInstallRequest<N>),
    /// A repeat of the Group Message 1 already answered: send this Group
    /// Message 2 again; no key changes.
    Retransmit(RsnTxFrame<N>),
}

/// Consume one Group Message 1 of a connected association and resolve the
/// unwrap of its key data through `unwrap`.
pub async fn process_group_message1<const N: usize, U: AsyncRsnKeyUnwrap>(
    supplicant: &mut RsnConnectedSupplicant,
    frame: OwnedEapolFrame<N>,
    unwrap: &mut U,
) -> Result<RsnGroupMessage1Step<N>, RsnConnectedProcessError<U::Error>> {
    match supplicant
        .on_group_message1(frame)
        .map_err(RsnConnectedProcessError::Supplicant)?
    {
        RsnConnectedAction::Retransmit(response) => Ok(RsnGroupMessage1Step::Retransmit(response)),
        RsnConnectedAction::UnwrapGroupKeyData(request) => {
            let unwrapped = unwrap
                .unwrap_key_data(request.kek(), request.wrapped_key_data())
                .await;
            supplicant
                .complete_group_key_data_unwrap(request, unwrapped)
                .map(RsnGroupMessage1Step::Install)
        }
    }
}

#[cfg(test)]
mod tests;
