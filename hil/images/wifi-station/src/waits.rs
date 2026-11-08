//! What the station services' waits look for in the network's publications:
//! an address over an up link, and the loss of the link up when a wait
//! starts. Both read the core's own view of the link in the same publication
//! as its address, so a wait for an address after a link loss never meets
//! the address of the link before it. They run on the group's hart, where
//! the network publishes.

use oer::wifi::{Configuration, DhcpState, Ipv4, LinkSnapshot, LinkState, WatchRefused};

/// The address a wait for one ends with: the interface's over an up link,
/// while the DHCP client holds it bound: a lease a server granted, confirmed
/// or extended, or an address the owner gave. A client still asking
/// (SELECTING, REQUESTING, an INIT-REBOOT no server answered yet) holds none
/// yet. Only this wait asks for a confirmed lease: the network's own
/// configuration and data keep using an unexpired lease while REBOOTING.
pub(crate) fn leased(configuration: &Configuration) -> Option<Ipv4> {
    if configuration.link.state != LinkState::Up {
        return None;
    }
    match configuration.dhcp {
        None | Some(DhcpState::Bound | DhcpState::Renewing | DhcpState::Rebinding) => {
            configuration.ipv4
        }
        Some(DhcpState::Selecting | DhcpState::Requesting | DhcpState::Rebooting) => None,
    }
}

/// Whether `now` lost the link `start` saw up: it is down, or up in a later
/// epoch. A link down at the start is lost at once.
pub(crate) fn lost(start: LinkSnapshot, now: LinkSnapshot) -> bool {
    now.state == LinkState::Down || now.epoch != start.epoch
}

/// The network's publications a wait reads.
pub(crate) trait Published {
    /// The last publication; `None` once the network ended.
    ///
    /// # Errors
    /// [`WatchRefused`]: a publication holds it at this moment.
    fn configuration(&self) -> Result<Option<Configuration>, WatchRefused>;
    /// A wrapping count of publications.
    fn changes(&self) -> u32;
    /// Wait until the count is no longer `seen`.
    async fn changed(&self, seen: u32) -> u32;
}

/// Wait for the first publication in which `found` finds its answer;
/// `None` once the network ended. The count is read before each
/// publication, so one published after the read wakes the wait.
///
/// # Errors
/// [`WatchRefused`] from a read.
pub(crate) async fn until<T>(
    net: &impl Published,
    mut found: impl FnMut(&Configuration) -> Option<T>,
) -> Result<Option<T>, WatchRefused> {
    loop {
        let seen = net.changes();
        let Some(configuration) = net.configuration()? else {
            return Ok(None);
        };
        if let Some(answer) = found(&configuration) {
            return Ok(Some(answer));
        }
        net.changed(seen).await;
    }
}

#[cfg(target_arch = "riscv32")]
impl<P: oer::wifi::Produce + 'static, M: oer::wifi::TransmitMode, R: oer::wifi::ReceiveMode>
    Published for oer::wifi::Network<P, M, R>
{
    fn configuration(&self) -> Result<Option<Configuration>, WatchRefused> {
        oer::wifi::Network::configuration(self)
    }

    fn changes(&self) -> u32 {
        oer::wifi::Network::changes(self)
    }

    async fn changed(&self, seen: u32) -> u32 {
        oer::wifi::Network::changed(self, seen).await
    }
}
