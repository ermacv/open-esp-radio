#![no_std]
use oer_esp32s31_ieee80211_form::scope::commands::{Submission, SetupSender};
use oer_ieee80211_lower_mac::Ieee80211Capabilities;
pub fn enqueue<'id, Caps: Ieee80211Capabilities>(
    sender: SetupSender<'_, 'id, Caps, (), (), (), (), 1, 0, 4, 10>,
    submission: Submission<'id, (), ()>,
) {
    #[cfg(not(ui_reject))]
    let mut sender = sender.enable();
    #[cfg(ui_reject)]
    let mut sender = sender;
    sender.submit(submission.operation, submission.admission); // ui-error: E0599
}
