//! Active channel geometry shared by AP verification and passive observers.
use crate::Result;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Geometry {
    pub frequency: u16,
    pub width: u16,
    pub center: u16,
}

impl Geometry {
    pub fn parse(info: &str) -> Result<Self> {
        let line = info
            .lines()
            .map(str::trim)
            .find(|line| line.starts_with("channel "))
            .ok_or("interface did not report a channel")?;
        let frequency = line
            .split_once('(')
            .and_then(|(_, tail)| tail.split_whitespace().next())
            .ok_or("interface omitted primary frequency")?
            .parse()?;
        let value = |key: &str| -> Result<u16> {
            Ok(line
                .split_once(key)
                .and_then(|(_, tail)| tail.split_whitespace().next())
                .ok_or("interface omitted channel geometry")?
                .parse()?)
        };
        let geometry = Self {
            frequency,
            width: value("width:")?,
            center: value("center1:")?,
        };
        geometry.iw_width()?;
        Ok(geometry)
    }

    pub fn iw_width(self) -> Result<&'static str> {
        match (
            self.width,
            i32::from(self.center) - i32::from(self.frequency),
        ) {
            (20, 0) => Ok("HT20"),
            (40, 10) => Ok("HT40+"),
            (40, -10) => Ok("HT40-"),
            _ => Err("unsupported or inconsistent monitor channel geometry".into()),
        }
    }
}

pub fn verify_ap_capabilities(
    channel_number: u8,
    ht40_above: Option<bool>,
    he: bool,
    info: &str,
) -> Result<()> {
    let channel_line = |number: u8| {
        let marker = format!("[{number}]");
        info.lines().find(|line| {
            line.split_whitespace().any(|word| word == marker)
                && line
                    .split_whitespace()
                    .nth(1)
                    .and_then(|word| word.parse::<f64>().ok())
                    == Some(f64::from(2407 + u16::from(number) * 5))
        })
    };
    let channel = channel_line(channel_number)
        .ok_or("AP radio PHY does not advertise the requested channel")?;
    if channel.contains("disabled") || channel.contains("no IR") {
        return Err(
            "AP radio regulatory state prohibits AP operation on the requested channel".into(),
        );
    }
    if !info.lines().any(|line| line.trim() == "* AP")
        || !info
            .lines()
            .any(|line| matches!(line.trim(), "HT20" | "HT20/HT40"))
    {
        return Err("radio does not advertise the required HT AP capability".into());
    }
    if he
        && !info.lines().any(|line| {
            line.contains("HE Iftypes:") && line.split([' ', ',', '\t']).any(|word| word == "AP")
        })
    {
        return Err("radio does not advertise HE support for the AP interface type".into());
    }
    if let Some(above) = ht40_above {
        if !info.contains("HT20/HT40") {
            return Err("radio does not support HT40".into());
        }
        let secondary = if above {
            channel_number
                .checked_add(4)
                .filter(|channel| *channel <= 13)
                .ok_or("invalid HT40 secondary channel")?
        } else {
            channel_number
                .checked_sub(4)
                .filter(|channel| *channel >= 1)
                .ok_or("invalid HT40 secondary channel")?
        };
        if !channel_line(secondary)
            .is_some_and(|line| !line.contains("disabled") && !line.contains("no IR"))
        {
            return Err("AP radio cannot use the HT40 secondary channel".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ht20_does_not_require_ht40_and_channel_numbers_are_band_specific() {
        let caps = "* AP\n HT20\n * 2472.0 MHz [13] (20 dBm)\n * 2452.0 MHz [9] (20 dBm)\n";
        verify_ap_capabilities(13, None, false, caps).unwrap();
        assert!(verify_ap_capabilities(13, Some(false), false, caps).is_err());
        let other_band = caps.replace("2472.0", "6015.0");
        assert!(verify_ap_capabilities(13, None, false, &other_band).is_err());
    }
    #[test]
    fn observer_retains_twenty_mhz_and_both_secondary_directions() {
        for (line, width) in [
            (
                "channel 13 (2472 MHz), width: 20 MHz, center1: 2472 MHz",
                "HT20",
            ),
            (
                "channel 3 (2422 MHz), width: 40 MHz, center1: 2432 MHz",
                "HT40+",
            ),
            (
                "channel 6 (2437 MHz), width: 40 MHz, center1: 2427 MHz",
                "HT40-",
            ),
        ] {
            assert_eq!(Geometry::parse(line).unwrap().iw_width().unwrap(), width);
        }
    }
}
