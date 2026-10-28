//! Stable action identities for calibration timing. Bit 7 marks the first
//! occurrence of an RX action in its retained execution region, not cache state.
use crate::{
    calibration::estimator::PhyDcIqAction as Iq, channel::PhyChipChannelAction as Channel,
};

pub(crate) fn channel(action: &Channel) -> u8 {
    match action {
        Channel::SetAgc { .. } => 1,
        Channel::ConfigureForceTxRx { .. } => 2,
        Channel::SetBbpllCalibration { .. } => 3,
        Channel::Temperature(_) => 4,
        Channel::StartFrequencySwitch { .. } => 5,
        Channel::DelayMicros { .. } => 6,
        Channel::ClearFrequencySwitch => 7,
        Channel::AwaitFrequencyReadyEdge { .. } => 8,
        Channel::ConfigureNrx { .. } => 9,
        Channel::ConfigureBssCbw { .. } => 10,
        Channel::ConfigureRxCompensation => 11,
        Channel::WriteI2c { .. } => 12,
        Channel::CalculateTxGain(_) => 13,
        Channel::PublishTxGain(_) => 14,
        Channel::ReadI2c { .. } => 15,
        Channel::PublishTxCapCommandMemory { .. } => 16,
        Channel::ConfigureChannelCbw { .. } => 17,
        Channel::ClearDcMemory => 18,
        Channel::Complete(_) | Channel::Failed(_) => 0,
    }
}

pub fn name(channel: bool, tag: u8) -> oer_diagnostics::Name<64> {
    let tag = tag & 0x7f;
    if channel {
        match tag {
            1 => oer_diagnostics::name!("SetAgc"; 64),
            2 => oer_diagnostics::name!("ConfigureForceTxRx"; 64),
            3 => oer_diagnostics::name!("SetBbpllCalibration"; 64),
            4 => oer_diagnostics::name!("Temperature"; 64),
            5 => oer_diagnostics::name!("StartFrequencySwitch"; 64),
            6 => oer_diagnostics::name!("DelayMicros"; 64),
            7 => oer_diagnostics::name!("ClearFrequencySwitch"; 64),
            8 => oer_diagnostics::name!("AwaitFrequencyReadyEdge"; 64),
            9 => oer_diagnostics::name!("ConfigureNrx"; 64),
            10 => oer_diagnostics::name!("ConfigureBssCbw"; 64),
            11 => oer_diagnostics::name!("ConfigureRxCompensation"; 64),
            12 => oer_diagnostics::name!("WriteI2c"; 64),
            13 => oer_diagnostics::name!("CalculateTxGain"; 64),
            14 => oer_diagnostics::name!("PublishTxGain"; 64),
            15 => oer_diagnostics::name!("ReadI2c"; 64),
            16 => oer_diagnostics::name!("PublishTxCapCommandMemory"; 64),
            17 => oer_diagnostics::name!("ConfigureChannelCbw"; 64),
            18 => oer_diagnostics::name!("ClearDcMemory"; 64),
            _ => oer_diagnostics::name!("Terminal"; 64),
        }
    } else {
        match tag {
            1 => oer_diagnostics::name!("PrepareDcControlRestore"; 64),
            2 => oer_diagnostics::name!("RestoreDcControl"; 64),
            3 => oer_diagnostics::name!("ConfigureLimits"; 64),
            4 => oer_diagnostics::name!("EnableIqCorrection"; 64),
            5 => oer_diagnostics::name!("Dc.ConfigureRegisters"; 64),
            6 => oer_diagnostics::name!("Dc.Rfpll"; 64),
            7 => oer_diagnostics::name!("Dc.ConfigurePbusDebugMode"; 64),
            8 => oer_diagnostics::name!("Dc.ForcePbus"; 64),
            9 => oer_diagnostics::name!("Dc.ConfigureClock"; 64),
            10 => oer_diagnostics::name!("Dc.ReadPbus"; 64),
            11 => oer_diagnostics::name!("Dc.I2c"; 64),
            12 => oer_diagnostics::name!("Dc.ConfigurePbusWorkMode"; 64),
            13 => oer_diagnostics::name!("Dc.DelayMicros"; 64),
            14 => oer_diagnostics::name!("Dc.ConfigurePbusWorkModePulse"; 64),
            15 => oer_diagnostics::name!("Dc.ClearPbusWorkModePulse"; 64),
            16 => oer_diagnostics::name!("Calibration.PrepareControlRestore"; 64),
            17 => oer_diagnostics::name!("Calibration.ReadPbus"; 64),
            18 => oer_diagnostics::name!("Calibration.ForcePbus"; 64),
            19 => oer_diagnostics::name!("Calibration.DelayMicros"; 64),
            20 => oer_diagnostics::name!("Calibration.RestoreControl"; 64),
            21 => oer_diagnostics::name!("DcIq.Configure"; 64),
            22 => oer_diagnostics::name!("DcIq.SetEnable"; 64),
            23 => oer_diagnostics::name!("DcIq.DelayMicros"; 64),
            24 => oer_diagnostics::name!("DcIq.AwaitReadinessEdge"; 64),
            25 => oer_diagnostics::name!("DcIq.ReadAccumulators"; 64),
            26 => oer_diagnostics::name!("Publish.ConfigurePbusDebugMode"; 64),
            27 => oer_diagnostics::name!("Publish.ForcePbus"; 64),
            28 => oer_diagnostics::name!("Publish.ConfigureClock"; 64),
            29 => oer_diagnostics::name!("Publish.ProgramEntry"; 64),
            30 => oer_diagnostics::name!("Publish.ConfigurePbusWorkMode"; 64),
            31 => oer_diagnostics::name!("Publish.DelayMicros"; 64),
            32 => oer_diagnostics::name!("Publish.ConfigurePbusWorkModePulse"; 64),
            33 => oer_diagnostics::name!("Publish.ClearPbusWorkModePulse"; 64),
            34 => oer_diagnostics::name!("Dc.I2c.Prepare"; 64),
            _ => oer_diagnostics::name!("Terminal"; 64),
        }
    }
}
