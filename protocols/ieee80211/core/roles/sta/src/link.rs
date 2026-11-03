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
//! A group key's replay lane is switched only by that key's installation
//! terminal: the holder applies the terminal to its replay epoch
//! ([`LinkHandover::group_terminal`]) before it hands the terminal to the
//! station.
//!
//! [`Station::eapol`]: crate::machine::Station::eapol
//! [`Station::receive`]: crate::machine::Station::receive
//! [`Station::link_returned`]: crate::machine::Station::link_returned

use oer_protocols_ieee80211_contracts_port::{
    Ieee80211Capabilities, LayoutError, RxMetadata, TxBufferClass, TxBufferMemory,
};
use oer_protocols_ieee80211_core_upper_mac::ccmp::{
    CcmpRxReplayEpoch, CcmpTxPacketNumber, InstalledCcmpTxKey,
};
use oer_protocols_ieee80211_core_upper_mac::receive::{AdmittedData, Integrity, RxDropReason};
use oer_protocols_ieee80211_core_upper_mac::sequence::SequenceCounter;
use oer_protocols_ieee80211_wire::address::{Bssid, OwnAddress};
use oer_protocols_ieee80211_wire::data::MsduParts;
use oer_protocols_ieee80211_wire::eapol::PAE_GROUP_ADDRESS;
use oer_protocols_ieee80211_wire::ethertype::ETHER_TYPE_EAPOL;
use oer_protocols_ieee80211_wire::frame::{DecodedHeader, MacHeader};
use oer_protocols_ieee80211_wire::qos::WmmUserPriority;
pub use oer_protocols_ieee80211_wire::sequence::SequenceNumber;
use oer_protocols_radio_port::PoolPath;

use crate::data::MsduDelivery;
use crate::data::plane::{DataLink, TxWindow};
use crate::epoch::LinkEpoch;
use crate::machine::{
    EapolProtection, StaEapol, StaFrame, StaInput, StaReceived, StaRxError, StaTokens, StaTransmit,
    TxRefusal, sta_frame,
};
use crate::receive::{Received, classify};

/// The space a station frame's Sequence Number comes from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkSpace {
    /// The station's own space: its management frames, its non-QoS data
    /// and EAPOL, and every data MPDU of a non-QoS BSS.
    Station,
    /// A TID's space of a QoS BSS, which the data of that TID shares.
    Tid(WmmUserPriority),
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

/// A pairwise key whose transmit packet numbers the link draws from.
pub trait TxKey {
    /// The key's transmit packet numbers.
    fn packet_numbers(&self) -> &CcmpTxPacketNumber;
}

impl<K> TxKey for InstalledCcmpTxKey<K> {
    fn packet_numbers(&self) -> &CcmpTxPacketNumber {
        InstalledCcmpTxKey::packet_numbers(self)
    }
}

/// The link's transmit numbers: the station's own sequence space, each
/// TID's space on a QoS BSS and the pairwise key `P` of a protected link,
/// whose packet numbers every protected MPDU of the link draws from.
pub struct TxNumbers<P> {
    station: SequenceCounter,
    tids: Option<[SequenceCounter; 8]>,
    key: Option<P>,
}

impl<P> TxNumbers<P> {
    pub(crate) const fn new(
        station: SequenceCounter,
        tids: Option<[SequenceCounter; 8]>,
        key: Option<P>,
    ) -> Self {
        Self { station, tids, key }
    }

    /// Whether the link is a QoS BSS's: a sequence space per TID.
    pub const fn qos(&self) -> bool {
        self.tids.is_some()
    }

    /// The link's pairwise key; `None` on an open link.
    pub const fn key(&self) -> Option<&P> {
        self.key.as_ref()
    }

    /// The next Sequence Number of data of `tid`: its TID's on a QoS BSS,
    /// the station's own otherwise.
    pub fn next(&self, tid: WmmUserPriority) -> SequenceNumber {
        match &self.tids {
            Some(tids) => tid.select(tids).peek(),
            None => self.station.peek(),
        }
    }

    /// The counter data of `tid` takes its numbers from, and the key its
    /// packet numbers come from.
    pub(crate) fn data_and_key(
        &mut self,
        tid: WmmUserPriority,
    ) -> (&mut SequenceCounter, Option<&P>) {
        let counter = match &mut self.tids {
            Some(tids) => tid.select_mut(tids),
            None => &mut self.station,
        };
        (counter, self.key.as_ref())
    }

    /// The counter of `space`; `None` for a TID of a non-QoS BSS.
    fn space(&mut self, space: LinkSpace) -> Option<&mut SequenceCounter> {
        match (space, &mut self.tids) {
            (LinkSpace::Station, _) => Some(&mut self.station),
            (LinkSpace::Tid(tid), Some(tids)) => Some(tid.select_mut(tids)),
            (LinkSpace::Tid(_), None) => None,
        }
    }

    /// Number one station frame of the link: a frame the station left to
    /// the link ([`FrameNumber::Link`]) takes the next Sequence Number of
    /// its space, written into its MAC header in its buffer and into its
    /// frame description; any other frame stays as the station wrote it.
    /// The number is taken whether or not the frame goes later, as the
    /// station's own numbering does. Its packet number is the caller's to
    /// draw from [`Self::key`], once its rate is known.
    ///
    /// # Errors
    ///
    /// A TID's space on a non-QoS BSS ([`TxRefusal::LinkChanged`]), or a
    /// buffer whose memory or header no longer holds the frame
    /// ([`TxRefusal::Layout`]).
    pub fn number<T: StaTokens, K: Ieee80211Capabilities>(
        &mut self,
        transmit: &mut StaTransmit<'_, T, K>,
    ) -> Result<(), TxRefusal> {
        self.number_frame(
            transmit.number,
            &mut transmit.frame,
            &mut transmit.buffer,
            transmit.plan,
        )
    }

    /// Number one station frame of the link from its parts, as
    /// [`Self::number`] does for a whole transmission: the frame's
    /// description `frame`, the `buffer` holding it, its rate `plan` and who
    /// numbers it (`number`).
    ///
    /// # Errors
    ///
    /// As [`Self::number`].
    pub fn number_frame<K: Ieee80211Capabilities, C: TxBufferClass>(
        &mut self,
        number: FrameNumber,
        frame: &mut StaFrame<K, C>,
        buffer: &mut impl TxBufferMemory,
        plan: crate::machine::StaRatePlan,
    ) -> Result<(), TxRefusal> {
        let FrameNumber::Link { space, .. } = number else {
            return Ok(());
        };
        let sequence = self.space(space).ok_or(TxRefusal::LinkChanged)?.take();
        renumber(frame, buffer, plan, sequence)
    }

    pub(crate) fn into_parts(self) -> (SequenceCounter, Option<[SequenceCounter; 8]>, Option<P>) {
        (self.station, self.tids, self.key)
    }
}

/// Write `sequence` into the Sequence Control of the frame `buffer` holds
/// and describe the frame again from its bytes.
fn renumber<K: Ieee80211Capabilities, C: TxBufferClass>(
    frame: &mut StaFrame<K, C>,
    buffer: &mut impl TxBufferMemory,
    plan: crate::machine::StaRatePlan,
    sequence: SequenceNumber,
) -> Result<(), TxRefusal> {
    let length = match frame {
        StaFrame::Individual(frame) => frame.frame().len().get(),
        StaFrame::Group(frame) => frame.frame().len().get(),
        // A control frame has no Sequence Control.
        StaFrame::BlockAckRequest { .. } => return Ok(()),
    };
    let short = TxRefusal::Layout(LayoutError::StorageTooShort);
    let bytes = buffer
        .bytes_mut()
        .map_err(|_| TxRefusal::Layout(LayoutError::BufferUnavailable))?;
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
    protected: bool,
    replay: Option<CcmpRxReplayEpoch>,
    /// The station's RSN failed on an EAPOL frame of the link: the link
    /// gives no more data until it returns.
    faulted: bool,
}

impl RxLink {
    pub(crate) const fn new(
        address: OwnAddress,
        bssid: Bssid,
        protected: bool,
        replay: Option<CcmpRxReplayEpoch>,
    ) -> Self {
        Self {
            address,
            bssid,
            protected,
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
    /// `eapol` and data to `deliver` while the link `carries` data. A frame
    /// refused for a typed reason keeps the MSDUs delivered before it.
    pub(crate) fn receive<'f>(
        &mut self,
        header: DecodedHeader<'f>,
        frame: &'f [u8],
        integrity: Integrity,
        carries: bool,
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
        for msdu in msdus(&admitted, self.address, self.protected)? {
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
                    if !carries || self.faulted {
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
/// the profile protects or not (`protected`): an unprotected EAPOL frame of
/// a protected link that names neither the station nor the PAE group goes
/// nowhere (`None`), and unprotected data of a protected link is refused.
pub(crate) fn msdus<'f>(
    admitted: &AdmittedData<'f>,
    address: OwnAddress,
    protected: bool,
) -> Result<impl Iterator<Item = Result<Option<LinkMsdu<'f>>, StaRxError>> + 'f, StaRxError> {
    let frame_protected = admitted.protected();
    Ok(admitted.msdus()?.map(move |parts| {
        let parts = parts?;
        if parts.ether_type == ETHER_TYPE_EAPOL {
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
pub struct LinkHandover<'id, Path: PoolPath, P> {
    epoch: LinkEpoch<'id, Path>,
    data: Option<DataLink>,
    numbers: TxNumbers<P>,
    rx: RxLink,
    /// The link carries data now, as the station last said.
    carries: bool,
}

impl<'id, Path: PoolPath, P> LinkHandover<'id, Path, P> {
    pub(crate) const fn new(
        epoch: LinkEpoch<'id, Path>,
        data: Option<DataLink>,
        numbers: TxNumbers<P>,
        rx: RxLink,
    ) -> Self {
        Self {
            epoch,
            data,
            numbers,
            rx,
            carries: false,
        }
    }

    /// The connection the link belongs to.
    pub const fn epoch(&self) -> LinkEpoch<'id, Path> {
        self.epoch
    }

    /// Whether the link carries data now: it is held from its handover to
    /// the station's first [`LinkAction::Release`], and from each
    /// [`LinkAction::Hold`] to the next release.
    pub const fn carries(&self) -> bool {
        self.carries
    }

    /// Take a [`LinkAction::Hold`] or [`LinkAction::Release`].
    pub fn hold(&mut self, hold: bool) {
        self.carries = !hold;
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
    pub const fn numbers(&self) -> &TxNumbers<P> {
        &self.numbers
    }

    /// The link's transmit numbers, to number a frame from.
    pub fn numbers_mut(&mut self) -> &mut TxNumbers<P> {
        &mut self.numbers
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
    pub fn receive<'f, M: RxMetadata>(
        &mut self,
        frame: &'f [u8],
        meta: &M,
        deliver: impl FnMut(MsduDelivery<'f>),
        eapol: impl FnMut(&'f [u8], EapolProtection) -> Result<StaEapol, StaRxError>,
    ) -> LinkRx {
        let header = match classify(frame) {
            Ok(Received::Data(header)) => header,
            Ok(Received::Management(_)) | Err(_) => return LinkRx::Station,
        };
        LinkRx::Taken(self.rx.receive(
            header,
            frame,
            crate::data::integrity(meta.common()),
            self.carries,
            deliver,
            eapol,
        ))
    }

    /// Apply a group key's terminal to the link's replay epoch before the
    /// station takes it: an installation, replacement or selection of the
    /// link's connection switches its key ID's replay lane to the new key
    /// (the old installation keeps its lane for the station), a replacement
    /// of another key ID ends that ID's lane, and the removal of an active
    /// key ends its lane. Any other input is left as it is.
    pub fn group_terminal<T: StaTokens<EpochPath = Path>, K>(
        &mut self,
        input: &mut StaInput<'id, T, K>,
    ) {
        let lane = match input {
            StaInput::GroupKeyInstalled { pending, .. } => GroupLane::Installed(pending),
            StaInput::GroupKeySelected { pending, .. } => GroupLane::Selected(pending),
            StaInput::GroupKeyReplaced { pending, .. } => GroupLane::Replaced(pending),
            StaInput::GroupKeyRemoved { pending, .. } => GroupLane::Removed(pending),
            _ => return,
        };
        self.group_lane(lane);
    }

    /// Apply the terminal of the group change `lane` names to the link's
    /// replay epoch, as [`Self::group_terminal`] does for the station's
    /// input: the pending token of the change, before it goes back to the
    /// station with its terminal.
    pub fn group_lane(&mut self, lane: GroupLane<'_, 'id, Path>) {
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
    /// (`protected`), QoS sequence spaces when `qos`, the pairwise `key`
    /// its protected frames draw packet numbers from, and the data plane's
    /// state of it.
    #[cfg(any(test, feature = "test-support"))]
    pub fn for_test(
        sequence: u64,
        address: OwnAddress,
        bssid: Bssid,
        protected: bool,
        qos: bool,
        key: Option<P>,
        data: Option<DataLink>,
    ) -> Self {
        Self::new(
            LinkEpoch::for_test(sequence),
            data,
            TxNumbers::new(
                SequenceCounter::new(SequenceNumber::ZERO),
                qos.then_some([SequenceCounter::new(SequenceNumber::ZERO); 8]),
                key,
            ),
            RxLink::new(address, bssid, protected, None),
        )
    }

    /// Give the link back to the station: what it needs to number its
    /// frames again.
    pub fn into_return(self) -> LinkReturn<P> {
        LinkReturn {
            numbers: self.numbers,
            rx: self.rx,
        }
    }
}

/// The link as its holder gives it back
/// ([`Station::link_returned`](crate::machine::Station::link_returned)): its
/// numbers, which go on in the station, and its receive admission.
pub struct LinkReturn<P> {
    pub(crate) numbers: TxNumbers<P>,
    pub(crate) rx: RxLink,
}

/// The pending token of one group key change whose terminal the link's
/// holder applies to the link's replay epoch ([`LinkHandover::group_lane`]).
pub enum GroupLane<'a, 'id, Path: PoolPath> {
    /// An installation.
    Installed(&'a mut crate::machine::PendingGroupKey<'id, Path>),
    /// A selection of a retained installation.
    Selected(&'a mut crate::machine::PendingGroupSelection<'id, Path>),
    /// A replacement.
    Replaced(&'a mut crate::machine::PendingGroupReplacement<'id, Path>),
    /// A removal.
    Removed(&'a crate::machine::PendingGroupRemoval<'id, Path>),
}

/// What the station tells its link's holder.
#[expect(
    clippy::large_enum_variant,
    reason = "the link moves by value from the station to its holder once per connection; the station has no allocator"
)]
pub enum LinkAction<'id, Path: PoolPath, P> {
    /// Take the link: from now on the holder numbers the link's frames and
    /// admits its data. It carries no data before a [`Self::Release`].
    HandOver(LinkHandover<'id, Path, P>),
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
}

#[cfg(test)]
mod tests;
