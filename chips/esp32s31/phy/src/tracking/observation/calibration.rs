//! Stable action identities for calibration timing. Bit 7 marks the first
//! occurrence of an RX action in its retained execution region, not cache state.
use crate::{
    calibration::estimator::PhyDcIqAction as Iq,
    channel::PhyChipChannelAction as Channel,
    rx::{
        dc_offset::PhyRxDcMinimumAction as Minimum,
        gain::{PhyRxGainInitAction as Init, PhyRxGainPublishAction as Publish},
        gain_calibration::{PhyRxDcCalibrationAction as Calibration, PhyRxGainDcAction as Dc},
    },
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

pub(crate) fn rx(action: &Init) -> u8 {
    match action {
        Init::PrepareDcControlRestore => 1,
        Init::RestoreDcControl => 2,
        Init::ConfigureLimits { .. } => 3,
        Init::EnableIqCorrection => 4,
        Init::Dc(action) => match action {
            Dc::ConfigureRegisters { .. } => 5,
            Dc::Rfpll(_) => 6,
            Dc::ConfigurePbusDebugMode => 7,
            Dc::ForcePbus { .. } => 8,
            Dc::ConfigureClock { .. } => 9,
            Dc::ReadPbus { .. } => 10,
            Dc::I2c(_) => 11,
            Dc::ConfigurePbusWorkMode => 12,
            Dc::DelayMicros { .. } => 13,
            Dc::ConfigurePbusWorkModePulse => 14,
            Dc::ClearPbusWorkModePulse => 15,
            Dc::Calibration(action) => match action {
                Calibration::PrepareControlRestore => 16,
                Calibration::ReadPbus { .. } => 17,
                Calibration::ForcePbus(_) => 18,
                Calibration::DelayMicros { .. } => 19,
                Calibration::RestoreControl => 20,
                Calibration::Minimum(Minimum::DcIq(action)) => match action {
                    Iq::Configure(_) => 21,
                    Iq::SetEnable { .. } => 22,
                    Iq::DelayMicros { .. } => 23,
                    Iq::AwaitReadinessEdge { .. } => 24,
                    Iq::ReadAccumulators(_) => 25,
                    Iq::Complete(_) | Iq::Failed(_) => 0,
                },
                Calibration::Complete(_)
                | Calibration::Failed(_)
                | Calibration::Minimum(Minimum::Complete(_) | Minimum::Failed(_)) => 0,
            },
            Dc::Complete | Dc::Failed(_) => 0,
        },
        Init::Publish(action) => match action {
            Publish::ConfigurePbusDebugMode { .. } => 26,
            Publish::ForcePbus { .. } => 27,
            Publish::ConfigureClock { .. } => 28,
            Publish::ProgramEntry { .. } => 29,
            Publish::ConfigurePbusWorkMode { .. } => 30,
            Publish::DelayMicros { .. } => 31,
            Publish::ConfigurePbusWorkModePulse { .. } => 32,
            Publish::ClearPbusWorkModePulse { .. } => 33,
            Publish::Complete(_) | Publish::Failed(_) => 0,
        },
        Init::Complete | Init::Failed(_) => 0,
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
            _ => "Terminal",
        }
    }
}
