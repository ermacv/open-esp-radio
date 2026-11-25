//! The station's link with its access point, as the station hands it to
//! the holder that carries its data.
//!
//! A connected station hands its link over once ([`LinkHandover`], in
//! [`LinkAction::HandOver`]): its transmit numbers ([`TxNumbers`]: each
//! TID's QoS sequence space, the station's own space, which management
//! frames and non-QoS data share, and the pairwise key's CCMP packet
//! numbers), its receive admission ([`RxLink`]: own address, BSSID,
//! protection and the CCMP replay epoch) and the data plane's state of the
//! link ([`DataLink`]). From then on the holder numbers both the data and
//! the station's own frames ([`FrameNumber::Link`]) and admits received
//! data without the station: data MSDUs go to the network, EAPOL to the
//! station ([`Station::eapol`]), management frames to the station
//! ([`Station::receive`]). The station tells the holder when the link stops
//! and starts carrying data ([`LinkAction::Hold`], [`LinkAction::Release`]:
//! sleep, absences and the key confirmation), each change of a TID's
//! transmit agreement ([`LinkAction::Agreement`]) and when it wants the link
//! back ([`LinkAction::Return`]); the holder returns it ([`LinkReturn`],
//! [`Station::link_returned`]) with everything the station needs to number
//! its frames again.
//!
//! The pairwise key is the engine's: the station hands over whether the
//! link is protected, never the key, and the engine draws the packet number
//! of every protected frame.
//!
//! Before its ADDBA Request names a TID's starting Sequence Number, the
//! station asks the holder for the TID's next number
//! ([`LinkAction::Sequence`], [`Station::tx_sequence`]); the holder takes no
//! more data of that TID until the request comes or the link is released
//! again, so the number the request names is the next data MPDU's.
//!
//! A group key's replay lane is switched only by that key's installation
//! end: the holder applies the end to its replay epoch
//! ([`LinkHandover::group_terminal`]) before it hands the end to the
//! station.
//!
//! [`Station::eapol`]: crate::machine::Station::eapol
//! [`Station::receive`]: crate::machine::Station::receive
//! [`Station::link_returned`]: crate::machine::Station::link_returned
//! [`Station::tx_sequence`]: crate::machine::Station::tx_sequence

use oer_protocols_ieee802_wire::EtherType;
use oer_protocols_ieee80211_contracts_port::{Ieee80211Capabilities, RxMetadata, StorageTooShort};
use oer_protocols_ieee80211_core_upper_mac::ccmp::CcmpRxReplayEpoch;
use oer_protocols_ieee80211_core_upper_mac::receive::{AdmittedData, Integrity, RxDropReason};
use oer_protocols_ieee80211_core_upper_mac::sequence::SequenceCounter;
use oer_protocols_ieee80211_wire::address::{Bssid, OwnAddress};
use oer_protocols_ieee80211_wire::data::MsduParts;
use oer_protocols_ieee80211_wire::eapol::PAE_GROUP_ADDRESS;
use oer_protocols_ieee80211_wire::frame::{DecodedHeader, MacHeader};
use oer_protocols_ieee80211_wire::qos::WmmUserPriority;
use oer_protocols_ieee80211_wire::security::LinkProtection;
pub use oer_protocols_ieee80211_wire::sequence::SequenceNumber;

use crate::data::MsduDelivery;
use crate::epoch::LinkEpoch;
use crate::machine::{
    EapolProtection, StaCallbacks, StaEapol, StaFrame, StaInput, StaKeyOutcome, StaReceived,
    StaRxError, StaTransmit, StaTypes, Station, TxRefusal, sta_frame,
};
use crate::receive::{Received, sort};
use oer_protocols_ieee80211_core_datapath::numbers::{LinkSpace, TxNumbers};
use oer_protocols_ieee80211_core_datapath::plane::{DataLink, TxWindow};

/// Whether a lent link carries data: from its handover it is held until
/// the station releases it ([`LinkAction::Release`]), and held again at
/// each [`LinkAction::Hold`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Carriage {
    Carries,
    Held,
}

/// Who numbers a station frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameNumber {
    /// The station numbered it: before its link exists and after the link
    /// came back, and a frame without Sequence Control (a BlockAckReq).
    Open,
    /// The link's holder numbers it as it submits it: its Sequence Number
    /// from `space` and, `protected`, its CCMP packet number from the
    /// link's pairwise key. The station wrote zero in its place.
    Link {
        /// Its sequence space.
        space: LinkSpace,
        /// It goes under the link's pairwise key.
        protected: bool,
    },
}

/// Number one station frame of the link from `numbers`: a frame the
/// station left to the link ([`FrameNumber::Link`]) takes the next Sequence
/// Number of its space, written into its MAC header in its buffer and into
/// its frame description; any other frame stays as the station wrote it.
/// The number is taken whether or not the frame goes later, as the
/// station's own numbering does. Its packet number is the caller's to draw
/// from the link's key ([`LinkHandover::key`]), once its rate is known.
///
/// # Errors
///
/// A TID's space on a non-QoS BSS ([`TxRefusal::LinkChanged`]), or a
/// buffer whose memory or header no longer holds the frame
/// ([`TxRefusal::Layout`]).
pub fn number<K: Ieee80211Capabilities>(
    numbers: &mut TxNumbers,
    transmit: &mut StaTransmit<'_, K>,
) -> Result<(), TxRefusal> {
    number_frame(
        numbers,
        transmit.number,
        &mut transmit.frame,
        transmit.bytes,
        transmit.plan,
    )
}

/// Number one station frame of the link from its parts, as [`number`]
/// does for a whole transmission: the frame's description `frame`, the
/// `bytes` holding it, its rate `plan` and who numbers it (`number`).
///
/// # Errors
///
/// As [`number`].
pub fn number_frame<K: Ieee80211Capabilities>(
    numbers: &mut TxNumbers,
    number: FrameNumber,
    frame: &mut StaFrame<K>,
    bytes: &mut [u8],
    plan: crate::machine::StaRatePlan,
) -> Result<(), TxRefusal> {
    let FrameNumber::Link { space, .. } = number else {
        // The station numbered it.
        return Ok(());
    };
    let sequence = numbers.take(space).ok_or(TxRefusal::LinkChanged)?;
    renumber(frame, bytes, plan, sequence)
}

/// Write `sequence` into the Sequence Control of the frame `bytes` hold
/// and describe the frame again from its bytes.
fn renumber<K: Ieee80211Capabilities>(
    frame: &mut StaFrame<K>,
    bytes: &mut [u8],
    plan: crate::machine::StaRatePlan,
    sequence: SequenceNumber,
) -> Result<(), TxRefusal> {
    let length = match frame {
        StaFrame::Individual(frame) => frame.len().get(),
        StaFrame::Group(frame) => frame.len().get(),
        // A control frame has no Sequence Control.
        StaFrame::BlockAckRequest { .. } => return Ok(()),
    };
    let short = TxRefusal::Layout(StorageTooShort);
    let bytes = bytes.get_mut(..length).ok_or(short)?;
    let header = MacHeader::decode(bytes)
        .ok_or(short)?
        .with_sequence_control(sequence.sequence_control());
    header.encode(bytes).map_err(|_| short)?;
    *frame = sta_frame(bytes, plan).ok_or(short)?;
    Ok(())
}

/// The link's receive admission: the station's own address, the BSSID, how
/// the profile protects the link and the replay check of the association's
/// keys.
pub struct RxLink {
    address: OwnAddress,
    bssid: Bssid,
    protection: LinkProtection,
    replay: Option<CcmpRxReplayEpoch>,
    /// The station's RSN failed on an EAPOL frame of the link: the link
    /// gives no more data until it returns.
    faulted: bool,
}

impl RxLink {
    pub(crate) const fn new(
        address: OwnAddress,
        bssid: Bssid,
        protection: LinkProtection,
        replay: Option<CcmpRxReplayEpoch>,
    ) -> Self {
        Self {
            address,
            bssid,
            protection,
            replay,
            faulted: false,
        }
    }

    /// The station's own address.
    pub const fn address(&self) -> OwnAddress {
        self.address
    }

    /// Admit one received data frame of the link at `integrity`: the
    /// replay check passes it once, then each MSDU goes on, EAPOL to
    /// `eapol` and data to `deliver` while the link carries data. A frame
    /// refused for a typed reason keeps the MSDUs delivered before it.
    pub(crate) fn receive<'f>(
        &mut self,
        header: DecodedHeader<'f>,
        frame: &'f [u8],
        integrity: Integrity,
        carriage: Carriage,
        mut deliver: impl FnMut(MsduDelivery<'f>),
        mut eapol: impl FnMut(&'f [u8], EapolProtection) -> Result<StaEapol, StaRxError>,
    ) -> Result<StaReceived, StaRxError> {
        let admitted = AdmittedData::admit(
            header,
            self.address,
            self.bssid,
            integrity,
            self.replay.as_mut(),
        )?;
        let mut received = StaReceived::Data;
        for msdu in msdus(&admitted, self.address, self.protection)? {
            match msdu? {
                None => {}
                Some(LinkMsdu::Eapol {
                    payload,
                    protection,
                }) => {
                    if eapol(payload, protection)? == StaEapol::Faulted {
                        self.faulted = true;
                    }
                    received = StaReceived::Input;
                }
                Some(LinkMsdu::Data(parts)) => {
                    if carriage == Carriage::Held || self.faulted {
                        return Err(RxDropReason::NotConnected.into());
                    }
                    deliver(MsduDelivery::locate(frame, parts).ok_or(RxDropReason::Payload)?);
                }
            }
        }
        Ok(received)
    }

    pub(crate) fn into_replay(self) -> Option<CcmpRxReplayEpoch> {
        self.replay
    }

    /// The replay check, for a test to change its keys.
    #[cfg(test)]
    pub(crate) fn replay_mut(&mut self) -> Option<&mut CcmpRxReplayEpoch> {
        self.replay.as_mut()
    }
}

/// One MSDU of an admitted data frame, as the station's link takes it.
pub(crate) enum LinkMsdu<'f> {
    /// An EAPOL frame for the station.
    Eapol {
        /// Its EAPOL packet.
        payload: &'f [u8],
        /// How the MPDU that carried it was protected.
        protection: EapolProtection,
    },
    /// Data for the network.
    Data(MsduParts<'f>),
}

/// The MSDUs of `admitted`, a frame to the station at `address` of a link
/// the profile protects or not (`protection`): an unprotected EAPOL frame of
/// a protected link that names neither the station nor the PAE group goes
/// nowhere (`None`), and unprotected data of a protected link is refused.
pub(crate) fn msdus<'f>(
    admitted: &AdmittedData<'f>,
    address: OwnAddress,
    protection: LinkProtection,
) -> Result<impl Iterator<Item = Result<Option<LinkMsdu<'f>>, StaRxError>> + 'f, StaRxError> {
    let frame_protected = admitted.protected();
    let protected = protection == LinkProtection::Ccmp;
    Ok(admitted.msdus()?.map(move |parts| {
        let parts = parts?;
        if parts.ether_type == EtherType::EAPOL {
            if protected
                && !frame_protected
                && parts.destination != address.get()
                && parts.destination != PAE_GROUP_ADDRESS
            {
                return Ok(None);
            }
            return Ok(Some(LinkMsdu::Eapol {
                payload: parts.payload,
                protection: if frame_protected {
                    EapolProtection::Pairwise
                } else {
                    EapolProtection::Open
                },
            }));
        }
        if protected && !frame_protected {
            return Err(RxDropReason::Protection.into());
        }
        Ok(Some(LinkMsdu::Data(parts)))
    }))
}

/// What the link's holder did with a received frame.
#[derive(Debug, Eq, PartialEq)]
pub enum LinkRx {
    /// It is not a data frame of the link: the station takes it
    /// ([`Station::receive`](crate::machine::Station::receive)).
    Station,
    /// The link took it: what it was to the station, or why it was refused.
    Taken(Result<StaReceived, StaRxError>),
}

/// The link a station hands over at connect: its connection, the data
/// plane's state of it (`None` for a BSSID the station sends no data to),
/// its transmit numbers and its receive admission. The holder keeps it
/// until the station asks for it back ([`Self::into_return`]).
pub struct LinkHandover<P> {
    epoch: LinkEpoch,
    data: Option<DataLink>,
    numbers: TxNumbers,
    /// The holder's pairwise key, whose packet numbers the link's protected
    /// frames draw; `None` on an open connection.
    key: Option<P>,
    rx: RxLink,
    /// Whether the link carries data now, as the station last said.
    carriage: Carriage,
}

impl LinkHandover<()> {
    pub(crate) const fn new(
        epoch: LinkEpoch,
        data: Option<DataLink>,
        numbers: TxNumbers,
        rx: RxLink,
    ) -> Self {
        Self {
            epoch,
            data,
            numbers,
            key: None,
            rx,
            carriage: Carriage::Held,
        }
    }

    /// The link with the holder's pairwise `key`, which numbers its
    /// protected frames: the station hands over none, the key is the
    /// engine's. A link of an open connection takes `None`.
    pub fn with_key<P>(self, key: Option<P>) -> LinkHandover<P> {
        let Self {
            epoch,
            data,
            numbers,
            key: _none,
            rx,
            carriage,
        } = self;
        // The station hands over no key.
        LinkHandover {
            epoch,
            data,
            numbers,
            key,
            rx,
            carriage,
        }
    }

    /// Whether the link is protected: its holder adds its pairwise key
    /// ([`Self::with_key`]).
    pub const fn protection(&self) -> LinkProtection {
        self.rx.protection
    }
}

impl<P> LinkHandover<P> {
    /// The connection the link belongs to.
    pub const fn epoch(&self) -> LinkEpoch {
        self.epoch
    }

    /// Whether the link carries data now: it is held from its handover to
    /// the station's first [`LinkAction::Release`], and from each
    /// [`LinkAction::Hold`] to the next release.
    pub const fn carriage(&self) -> Carriage {
        self.carriage
    }

    /// Take a [`LinkAction::Hold`] ([`Carriage::Held`]) or
    /// [`LinkAction::Release`] ([`Carriage::Carries`]).
    pub fn carry(&mut self, carriage: Carriage) {
        self.carriage = carriage;
    }

    /// The data plane's state of the link, until the holder hands it to its
    /// plane.
    pub fn data_mut(&mut self) -> Option<&mut DataLink> {
        self.data.as_mut()
    }

    /// Hand the data plane's state of the link to the plane, once.
    pub fn take_data(&mut self) -> Option<DataLink> {
        self.data.take()
    }

    /// Record a [`LinkAction::Agreement`] in the data plane's state of the
    /// link while the holder keeps it; `false` once the plane holds it, and
    /// the plane takes the agreement itself.
    pub fn agreement(&mut self, tid: WmmUserPriority, window: Option<TxWindow>) -> bool {
        match self.data.as_mut() {
            Some(data) => {
                *tid.select_mut(&mut data.windows) = window;
                true
            }
            None => false,
        }
    }

    /// The link's transmit numbers.
    pub const fn numbers(&self) -> &TxNumbers {
        &self.numbers
    }

    /// The link's transmit numbers, to number a frame from.
    pub fn numbers_mut(&mut self) -> &mut TxNumbers {
        &mut self.numbers
    }

    /// The holder's pairwise key; `None` on an open connection.
    pub const fn key(&self) -> Option<&P> {
        self.key.as_ref()
    }

    /// The numbers and the key together, to draw a data MPDU's from.
    pub fn numbers_and_key(&mut self) -> (&mut TxNumbers, Option<&mut P>) {
        (&mut self.numbers, self.key.as_mut())
    }

    /// The link's receive admission.
    pub const fn rx(&self) -> &RxLink {
        &self.rx
    }

    /// The link's replay check, for a test to play received frames on.
    #[cfg(test)]
    pub(crate) fn replay_mut(&mut self) -> Option<&mut CcmpRxReplayEpoch> {
        self.rx.replay_mut()
    }

    /// Take one received frame with its `meta`: a data frame of the link is
    /// admitted here, its data MSDUs go to `deliver` while the link carries
    /// data and its EAPOL frames to `eapol`, the station's
    /// [`Station::eapol`](crate::machine::Station::eapol); any other frame is
    /// the station's.
    ///
    /// `header` is the frame's MAC header as its receive buffer decoded it
    /// ([`RxBuffer::header`](oer_protocols_ieee80211_contracts_port::RxBuffer::header)),
    /// `None` for one that does not decode, which is the station's.
    pub fn receive<'f, M: RxMetadata>(
        &mut self,
        header: Option<DecodedHeader<'f>>,
        meta: &M,
        deliver: impl FnMut(MsduDelivery<'f>),
        eapol: impl FnMut(&'f [u8], EapolProtection) -> Result<StaEapol, StaRxError>,
    ) -> LinkRx {
        let header = match header.map(sort) {
            Some(Received::Data(header)) => header,
            Some(Received::Management(_)) | None => return LinkRx::Station,
        };
        let frame = header.frame();
        LinkRx::Taken(self.rx.receive(
            header,
            frame,
            crate::data::integrity(meta.common()),
            self.carriage,
            deliver,
            eapol,
        ))
    }

    /// Apply a group key operation's end to the link's replay epoch before
    /// the station takes it: an installation, replacement or selection of
    /// the link's connection switches its key ID's replay lane to the new
    /// key (the old installation keeps its lane for the station), a
    /// replacement of another key ID ends that ID's lane, and the removal of
    /// an active key ends its lane. An installation, replacement or selection
    /// that expired switches nothing. Any other input is left as it is.
    pub fn group_terminal<T: StaTypes, K: Ieee80211Capabilities, C: StaCallbacks>(
        &mut self,
        station: &mut Station<'_, T, K, C>,
        input: &StaInput<'_, T, K>,
    ) {
        let StaInput::GroupKeyEnded { place, outcome } = input else {
            return;
        };
        match station.group_lane(*place) {
            // A removal ends its lane however it ended.
            Some(lane @ GroupLane::Removed(_)) => self.group_lane(lane),
            // An installation that expired took no lane.
            Some(lane) if *outcome == StaKeyOutcome::Done => self.group_lane(lane),
            _ => {}
        }
    }

    /// Apply the end of the group change `lane` names to the link's replay
    /// epoch, as [`Self::group_terminal`] does for the station's input: the
    /// station's record of the change, before the station takes its end.
    pub(crate) fn group_lane(&mut self, lane: GroupLane<'_>) {
        let Some(replay) = self.rx.replay.as_mut() else {
            return;
        };
        match lane {
            GroupLane::Installed(pending) => {
                if pending.epoch == self.epoch {
                    pending.activate(replay);
                }
            }
            GroupLane::Selected(pending) => {
                if pending.install.epoch == self.epoch {
                    pending.install.activate(replay);
                }
            }
            GroupLane::Replaced(pending) => {
                if pending.install.epoch == self.epoch {
                    if let Some(retired) = pending.retired_lane() {
                        replay.remove_group(retired);
                    }
                    pending.install.activate(replay);
                }
            }
            GroupLane::Removed(pending) => {
                if pending.active() {
                    replay.remove_group(pending.key_id);
                }
            }
        }
    }

    /// The station's RSN failed on an EAPOL frame of the link that its
    /// holder handed it: the link gives no more data until it returns.
    pub fn eapol_faulted(&mut self) {
        self.rx.faulted = true;
    }

    /// An open link of the connection `sequence` for a host test of its
    /// holder: own `address`, `bssid`, the profile's protection
    /// (`protection`), QoS sequence spaces for a `qos` peer, the holder's
    /// pairwise `key`, and the data plane's state of it.
    #[cfg(any(test, feature = "test-support"))]
    pub fn for_test(
        sequence: u64,
        address: OwnAddress,
        bssid: Bssid,
        protection: LinkProtection,
        qos: oer_protocols_ieee80211_wire::data::PeerQos,
        key: Option<P>,
        data: Option<DataLink>,
    ) -> Self {
        Self {
            epoch: LinkEpoch::for_test(sequence),
            data,
            numbers: TxNumbers::new(
                SequenceCounter::new(SequenceNumber::ZERO),
                (qos == oer_protocols_ieee80211_wire::data::PeerQos::Qos)
                    .then_some([SequenceCounter::new(SequenceNumber::ZERO); 8]),
            ),
            key,
            rx: RxLink::new(address, bssid, protection, None),
            carriage: Carriage::Held,
        }
    }

    /// Give the link back to the station: what it needs to number its
    /// frames again. The holder keeps its key.
    pub fn into_return(self) -> (LinkReturn, Option<P>) {
        let (station, tids) = self.numbers.into_parts();
        (
            LinkReturn {
                station,
                tids,
                rx: self.rx,
            },
            self.key,
        )
    }
}

/// The link as its holder gives it back
/// ([`Station::link_returned`](crate::machine::Station::link_returned)): its
/// sequence spaces, which go on in the station, and its receive admission.
pub struct LinkReturn {
    pub(crate) station: SequenceCounter,
    pub(crate) tids: Option<[SequenceCounter; 8]>,
    pub(crate) rx: RxLink,
}

/// The station's record of one group key change whose end the link's
/// holder applies to the link's replay epoch ([`LinkHandover::group_lane`]).
pub(crate) enum GroupLane<'a> {
    /// An installation.
    Installed(&'a mut crate::machine::PendingGroupKey),
    /// A selection of a retained installation.
    Selected(&'a mut crate::machine::PendingGroupSelection),
    /// A replacement.
    Replaced(&'a mut crate::machine::PendingGroupReplacement),
    /// A removal.
    Removed(&'a crate::machine::PendingGroupRemoval),
}

/// What the station tells its link's holder.
#[expect(
    clippy::large_enum_variant,
    reason = "the link moves by value from the station to its holder once per connection; the station has no allocator"
)]
pub enum LinkAction {
    /// Take the link: from now on the holder numbers the link's frames and
    /// admits its data. It carries no data before a [`Self::Release`].
    HandOver(LinkHandover<()>),
    /// The link carries no data for now: the station sleeps, is away on
    /// another channel or its keys are not confirmed yet.
    Hold,
    /// The link carries data again.
    Release,
    /// The transmit agreement of `tid` started, moved or ended.
    Agreement {
        /// Its TID.
        tid: WmmUserPriority,
        /// Its window; `None` once it ended.
        window: Option<TxWindow>,
    },
    /// Give the link back ([`LinkHandover::into_return`]).
    Return,
    /// Tell the next Sequence Number of `tid`
    /// ([`Station::tx_sequence`](crate::machine::Station::tx_sequence)) and
    /// take no more data of `tid` until the station's ADDBA Request of it
    /// comes or the next [`Self::Release`].
    Sequence(WmmUserPriority),
}

#[cfg(test)]
mod tests;
