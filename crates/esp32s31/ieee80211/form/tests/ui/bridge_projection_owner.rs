#![no_std]
use core::cell::Cell;
use oer_esp32s31_ieee80211_form::scope::commands::{
    bridge::{Held,ProjectExecution,TxImage}, GatePath,EnableKind,DisableKind,
};
use oer_ieee80211_lower_mac::{ExecutionPayload,GateIssuer,Ieee80211Capabilities};
use oer_radio_port::{Root,Child,Slot};

pub struct Payload<'a> { _short_non_send_tag:&'a Cell<u8>, image:TxImage }
pub struct View<'a> { image:&'a TxImage }
impl ExecutionPayload for Payload<'_> {
    type Execution<'a> = View<'a> where Self:'a;
    fn execution(&mut self)->View<'_> { View { image:&self.image } }
}
impl ProjectExecution for Payload<'_> {
    fn project(view:View<'_>)->TxImage { *view.image }
}
// The positive twin also instantiates the actual producer with a payload
// that cannot be 'static. A for<'a> GAT bound here incorrectly rejects it.
pub fn split<'id,'tag,Caps:Ieee80211Capabilities>(
    held:&mut Held<'id,Caps,(),(),(),Payload<'tag>,0,0,4,9,()>,
    issuer:GateIssuer<'id,GatePath>,
    enable:Slot<'id,EnableKind,Child<Root,8>>,
    disable:Slot<'id,DisableKind,Child<Root,9>>,
) {
    let _ = held.split(issuer,enable,disable);
}
pub fn project(payload:&mut Payload<'_>)->TxImage {
    #[cfg(ui_reject)]
    { Payload::project(payload) } // ui-error: E0308
    #[cfg(not(ui_reject))]
    { Payload::project(payload.execution()) }
}
