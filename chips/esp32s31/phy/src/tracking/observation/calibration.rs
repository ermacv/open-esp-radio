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

pub fn name(channel: bool, tag: u8) -> &'static str {
    let tag = tag & 0x7f;
    if channel {
        match tag {
            1 => "SetAgc",
            2 => "ConfigureForceTxRx",
            3 => "SetBbpllCalibration",
            4 => "Temperature",
            5 => "StartFrequencySwitch",
            6 => "DelayMicros",
            7 => "ClearFrequencySwitch",
            8 => "AwaitFrequencyReadyEdge",
            9 => "ConfigureNrx",
            10 => "ConfigureBssCbw",
            11 => "ConfigureRxCompensation",
            12 => "WriteI2c",
            13 => "CalculateTxGain",
            14 => "PublishTxGain",
            15 => "ReadI2c",
            16 => "PublishTxCapCommandMemory",
            17 => "ConfigureChannelCbw",
            18 => "ClearDcMemory",
            _ => "Terminal",
        }
    } else {
        match tag {
            1 => "PrepareDcControlRestore",
            2 => "RestoreDcControl",
            3 => "ConfigureLimits",
            4 => "EnableIqCorrection",
            5 => "Dc.ConfigureRegisters",
            6 => "Dc.Rfpll",
            7 => "Dc.ConfigurePbusDebugMode",
            8 => "Dc.ForcePbus",
            9 => "Dc.ConfigureClock",
            10 => "Dc.ReadPbus",
            11 => "Dc.I2c",
            12 => "Dc.ConfigurePbusWorkMode",
            13 => "Dc.DelayMicros",
            14 => "Dc.ConfigurePbusWorkModePulse",
            15 => "Dc.ClearPbusWorkModePulse",
            16 => "Calibration.PrepareControlRestore",
            17 => "Calibration.ReadPbus",
            18 => "Calibration.ForcePbus",
            19 => "Calibration.DelayMicros",
            20 => "Calibration.RestoreControl",
            21 => "DcIq.Configure",
            22 => "DcIq.SetEnable",
            23 => "DcIq.DelayMicros",
            24 => "DcIq.AwaitReadinessEdge",
            25 => "DcIq.ReadAccumulators",
            26 => "Publish.ConfigurePbusDebugMode",
            27 => "Publish.ForcePbus",
            28 => "Publish.ConfigureClock",
            29 => "Publish.ProgramEntry",
            30 => "Publish.ConfigurePbusWorkMode",
            31 => "Publish.DelayMicros",
            32 => "Publish.ConfigurePbusWorkModePulse",
            33 => "Publish.ClearPbusWorkModePulse",
            34 => "Dc.I2c.Prepare",
            _ => "Terminal",
        }
    }
}
