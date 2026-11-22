//! Golden memory images of the AES and AXI-DMA operations.
//!
//! Each case runs one operation on host memory of its parts, filled with
//! one value, and compares every part with the fill and the listed words
//! (byte offsets from the instance's base) replaced. The images were
//! recorded from the operations as merged in 850f10d2e and are written out
//! here, so a later rewrite cannot pass by reading back its own error.

use super::ops;
use oer_chips_esp32s31_pac::{aes, axi_dma, hp_sys_clkrst, instances};
use std::{vec, vec::Vec};

/// Compare `memory`, filled with `fill`, with the fill and the words that
/// `changed` lists for `part` replaced.
fn check(part: &str, fill: u32, memory: &[u32], changed: &[(&str, usize, u32)]) {
    for (index, got) in memory.iter().enumerate() {
        let want = changed
            .iter()
            .find(|(name, offset, _)| *name == part && offset / 4 == index)
            .map_or(fill, |&(_, _, value)| value);
        assert_eq!(
            *got,
            want,
            "{part} word {:#06x} from fill {fill:#010x}: {got:#010x}, expected {want:#010x}",
            index * 4
        );
    }
}

#[test]
fn disable_interrupt() {
    {
        let mut m_p: Vec<u32> = vec![0; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::disable_interrupt(&mut p);
        }
        check("p", 0, &m_p, &[]);
    }
    {
        let mut m_p: Vec<u32> = vec![u32::MAX; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::disable_interrupt(&mut p);
        }
        check("p", u32::MAX, &m_p, &[("p", 0x00b0, 0xfffffffe)]);
    }
}

#[test]
fn enable_interrupt() {
    {
        let mut m_p: Vec<u32> = vec![0; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::enable_interrupt(&mut p);
        }
        check("p", 0, &m_p, &[("p", 0x00b0, 0x00000001)]);
    }
    {
        let mut m_p: Vec<u32> = vec![u32::MAX; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::enable_interrupt(&mut p);
        }
        check("p", u32::MAX, &m_p, &[]);
    }
}

#[test]
fn disable_dma() {
    {
        let mut m_p: Vec<u32> = vec![0; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::disable_dma(&mut p);
        }
        check("p", 0, &m_p, &[]);
    }
    {
        let mut m_p: Vec<u32> = vec![u32::MAX; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::disable_dma(&mut p);
        }
        check("p", u32::MAX, &m_p, &[("p", 0x0090, 0xfffffffe)]);
    }
}

#[test]
fn decrypt128() {
    {
        let mut m_p: Vec<u32> = vec![0; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::decrypt128(&mut p);
        }
        check("p", 0, &m_p, &[("p", 0x0040, 0x00000004)]);
    }
    {
        let mut m_p: Vec<u32> = vec![u32::MAX; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::decrypt128(&mut p);
        }
        check("p", u32::MAX, &m_p, &[("p", 0x0040, 0xffffffec)]);
    }
}

#[test]
fn encrypt128() {
    {
        let mut m_p: Vec<u32> = vec![0; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::encrypt128(&mut p);
        }
        check("p", 0, &m_p, &[]);
    }
    {
        let mut m_p: Vec<u32> = vec![u32::MAX; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::encrypt128(&mut p);
        }
        check("p", u32::MAX, &m_p, &[("p", 0x0040, 0xffffffe8)]);
    }
}

#[test]
fn disable_pseudo_rounds() {
    {
        let mut m_p: Vec<u32> = vec![0; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::disable_pseudo_rounds(&mut p);
        }
        check("p", 0, &m_p, &[]);
    }
    {
        let mut m_p: Vec<u32> = vec![u32::MAX; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::disable_pseudo_rounds(&mut p);
        }
        check("p", u32::MAX, &m_p, &[("p", 0x00d0, 0xfffffffe)]);
    }
}

#[test]
fn hold() {
    {
        let mut m_p: Vec<u32> = vec![0; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::hold(&mut p);
        }
        check("p", 0, &m_p, &[]);
    }
    {
        let mut m_p: Vec<u32> = vec![u32::MAX; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::hold(&mut p);
        }
        check("p", u32::MAX, &m_p, &[("p", 0x00d4, 0xfffffffe)]);
    }
}

#[test]
fn start() {
    {
        let mut m_p: Vec<u32> = vec![0; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::start(&mut p);
        }
        check("p", 0, &m_p, &[("p", 0x0048, 0x00000001)]);
    }
    {
        let mut m_p: Vec<u32> = vec![u32::MAX; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::start(&mut p);
        }
        check("p", u32::MAX, &m_p, &[("p", 0x0048, 0x00000001)]);
    }
}

#[test]
fn finish_dma() {
    {
        let mut m_p: Vec<u32> = vec![0; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::finish_dma(&mut p);
        }
        check("p", 0, &m_p, &[("p", 0x00b8, 0x00000001)]);
    }
    {
        let mut m_p: Vec<u32> = vec![u32::MAX; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::finish_dma(&mut p);
        }
        check(
            "p",
            u32::MAX,
            &m_p,
            &[("p", 0x0090, 0xfffffffe), ("p", 0x00b8, 0x00000001)],
        );
    }
}

#[test]
fn write_key() {
    {
        let mut m_p: Vec<u32> = vec![0; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::write_key(
                &mut p,
                &[
                    0x0102_0304,
                    0x0506_0708,
                    0x090a_0b0c,
                    0x0d0e_0f10,
                    0x1112_1314,
                    0x1516_1718,
                    0x191a_1b1c,
                    0x1d1e_1f20,
                ],
            );
        }
        check(
            "p",
            0,
            &m_p,
            &[
                ("p", 0x0000, 0x01020304),
                ("p", 0x0004, 0x05060708),
                ("p", 0x0008, 0x090a0b0c),
                ("p", 0x000c, 0x0d0e0f10),
                ("p", 0x0010, 0x11121314),
                ("p", 0x0014, 0x15161718),
                ("p", 0x0018, 0x191a1b1c),
                ("p", 0x001c, 0x1d1e1f20),
            ],
        );
    }
    {
        let mut m_p: Vec<u32> = vec![u32::MAX; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::write_key(
                &mut p,
                &[
                    0x0102_0304,
                    0x0506_0708,
                    0x090a_0b0c,
                    0x0d0e_0f10,
                    0x1112_1314,
                    0x1516_1718,
                    0x191a_1b1c,
                    0x1d1e_1f20,
                ],
            );
        }
        check(
            "p",
            u32::MAX,
            &m_p,
            &[
                ("p", 0x0000, 0x01020304),
                ("p", 0x0004, 0x05060708),
                ("p", 0x0008, 0x090a0b0c),
                ("p", 0x000c, 0x0d0e0f10),
                ("p", 0x0010, 0x11121314),
                ("p", 0x0014, 0x15161718),
                ("p", 0x0018, 0x191a1b1c),
                ("p", 0x001c, 0x1d1e1f20),
            ],
        );
    }
}

#[test]
fn write_block() {
    {
        let mut m_p: Vec<u32> = vec![0; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::write_block(
                &mut p,
                &[0xa1a2_a3a4, 0xb1b2_b3b4, 0xc1c2_c3c4, 0xd1d2_d3d4],
            );
        }
        check(
            "p",
            0,
            &m_p,
            &[
                ("p", 0x0020, 0xa1a2a3a4),
                ("p", 0x0024, 0xb1b2b3b4),
                ("p", 0x0028, 0xc1c2c3c4),
                ("p", 0x002c, 0xd1d2d3d4),
            ],
        );
    }
    {
        let mut m_p: Vec<u32> = vec![u32::MAX; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::write_block(
                &mut p,
                &[0xa1a2_a3a4, 0xb1b2_b3b4, 0xc1c2_c3c4, 0xd1d2_d3d4],
            );
        }
        check(
            "p",
            u32::MAX,
            &m_p,
            &[
                ("p", 0x0020, 0xa1a2a3a4),
                ("p", 0x0024, 0xb1b2b3b4),
                ("p", 0x0028, 0xc1c2c3c4),
                ("p", 0x002c, 0xd1d2d3d4),
            ],
        );
    }
}

#[test]
fn write_iv() {
    {
        let mut m_p: Vec<u32> = vec![0; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::write_iv(
                &mut p,
                &[0x1111_2222, 0x3333_4444, 0x5555_6666, 0x7777_8888],
            );
        }
        check(
            "p",
            0,
            &m_p,
            &[
                ("p", 0x0050, 0x11112222),
                ("p", 0x0054, 0x33334444),
                ("p", 0x0058, 0x55556666),
                ("p", 0x005c, 0x77778888),
            ],
        );
    }
    {
        let mut m_p: Vec<u32> = vec![u32::MAX; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::write_iv(
                &mut p,
                &[0x1111_2222, 0x3333_4444, 0x5555_6666, 0x7777_8888],
            );
        }
        check(
            "p",
            u32::MAX,
            &m_p,
            &[
                ("p", 0x0050, 0x11112222),
                ("p", 0x0054, 0x33334444),
                ("p", 0x0058, 0x55556666),
                ("p", 0x005c, 0x77778888),
            ],
        );
    }
}

#[test]
fn start_dma_ecb_16() {
    {
        let mut m_p: Vec<u32> = vec![0; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::start_dma(&mut p, super::DmaBlockMode::Ecb, 16);
        }
        check(
            "p",
            0,
            &m_p,
            &[
                ("p", 0x0048, 0x00000001),
                ("p", 0x0090, 0x00000001),
                ("p", 0x0098, 0x00000001),
            ],
        );
    }
    {
        let mut m_p: Vec<u32> = vec![u32::MAX; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::start_dma(&mut p, super::DmaBlockMode::Ecb, 16);
        }
        check(
            "p",
            u32::MAX,
            &m_p,
            &[
                ("p", 0x0048, 0x00000001),
                ("p", 0x0094, 0xfffffff8),
                ("p", 0x0098, 0x00000001),
                ("p", 0x009c, 0xfffffffe),
            ],
        );
    }
}

#[test]
fn start_dma_ecb_1568() {
    {
        let mut m_p: Vec<u32> = vec![0; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::start_dma(&mut p, super::DmaBlockMode::Ecb, 1568);
        }
        check(
            "p",
            0,
            &m_p,
            &[
                ("p", 0x0048, 0x00000001),
                ("p", 0x0090, 0x00000001),
                ("p", 0x0098, 0x00000062),
            ],
        );
    }
    {
        let mut m_p: Vec<u32> = vec![u32::MAX; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::start_dma(&mut p, super::DmaBlockMode::Ecb, 1568);
        }
        check(
            "p",
            u32::MAX,
            &m_p,
            &[
                ("p", 0x0048, 0x00000001),
                ("p", 0x0094, 0xfffffff8),
                ("p", 0x0098, 0x00000062),
                ("p", 0x009c, 0xfffffffe),
            ],
        );
    }
}

#[test]
fn start_dma_cbc_16() {
    {
        let mut m_p: Vec<u32> = vec![0; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::start_dma(&mut p, super::DmaBlockMode::Cbc, 16);
        }
        check(
            "p",
            0,
            &m_p,
            &[
                ("p", 0x0048, 0x00000001),
                ("p", 0x0090, 0x00000001),
                ("p", 0x0094, 0x00000001),
                ("p", 0x0098, 0x00000001),
            ],
        );
    }
    {
        let mut m_p: Vec<u32> = vec![u32::MAX; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::start_dma(&mut p, super::DmaBlockMode::Cbc, 16);
        }
        check(
            "p",
            u32::MAX,
            &m_p,
            &[
                ("p", 0x0048, 0x00000001),
                ("p", 0x0094, 0xfffffff9),
                ("p", 0x0098, 0x00000001),
                ("p", 0x009c, 0xfffffffe),
            ],
        );
    }
}

#[test]
fn start_dma_cbc_1568() {
    {
        let mut m_p: Vec<u32> = vec![0; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::start_dma(&mut p, super::DmaBlockMode::Cbc, 1568);
        }
        check(
            "p",
            0,
            &m_p,
            &[
                ("p", 0x0048, 0x00000001),
                ("p", 0x0090, 0x00000001),
                ("p", 0x0094, 0x00000001),
                ("p", 0x0098, 0x00000062),
            ],
        );
    }
    {
        let mut m_p: Vec<u32> = vec![u32::MAX; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::start_dma(&mut p, super::DmaBlockMode::Cbc, 1568);
        }
        check(
            "p",
            u32::MAX,
            &m_p,
            &[
                ("p", 0x0048, 0x00000001),
                ("p", 0x0094, 0xfffffff9),
                ("p", 0x0098, 0x00000062),
                ("p", 0x009c, 0xfffffffe),
            ],
        );
    }
}

#[test]
fn start_dma_ofb_16() {
    {
        let mut m_p: Vec<u32> = vec![0; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::start_dma(&mut p, super::DmaBlockMode::Ofb, 16);
        }
        check(
            "p",
            0,
            &m_p,
            &[
                ("p", 0x0048, 0x00000001),
                ("p", 0x0090, 0x00000001),
                ("p", 0x0094, 0x00000002),
                ("p", 0x0098, 0x00000001),
            ],
        );
    }
    {
        let mut m_p: Vec<u32> = vec![u32::MAX; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::start_dma(&mut p, super::DmaBlockMode::Ofb, 16);
        }
        check(
            "p",
            u32::MAX,
            &m_p,
            &[
                ("p", 0x0048, 0x00000001),
                ("p", 0x0094, 0xfffffffa),
                ("p", 0x0098, 0x00000001),
                ("p", 0x009c, 0xfffffffe),
            ],
        );
    }
}

#[test]
fn start_dma_ofb_1568() {
    {
        let mut m_p: Vec<u32> = vec![0; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::start_dma(&mut p, super::DmaBlockMode::Ofb, 1568);
        }
        check(
            "p",
            0,
            &m_p,
            &[
                ("p", 0x0048, 0x00000001),
                ("p", 0x0090, 0x00000001),
                ("p", 0x0094, 0x00000002),
                ("p", 0x0098, 0x00000062),
            ],
        );
    }
    {
        let mut m_p: Vec<u32> = vec![u32::MAX; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::start_dma(&mut p, super::DmaBlockMode::Ofb, 1568);
        }
        check(
            "p",
            u32::MAX,
            &m_p,
            &[
                ("p", 0x0048, 0x00000001),
                ("p", 0x0094, 0xfffffffa),
                ("p", 0x0098, 0x00000062),
                ("p", 0x009c, 0xfffffffe),
            ],
        );
    }
}

#[test]
fn start_dma_ctr_16() {
    {
        let mut m_p: Vec<u32> = vec![0; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::start_dma(&mut p, super::DmaBlockMode::Ctr, 16);
        }
        check(
            "p",
            0,
            &m_p,
            &[
                ("p", 0x0048, 0x00000001),
                ("p", 0x0090, 0x00000001),
                ("p", 0x0094, 0x00000003),
                ("p", 0x0098, 0x00000001),
            ],
        );
    }
    {
        let mut m_p: Vec<u32> = vec![u32::MAX; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::start_dma(&mut p, super::DmaBlockMode::Ctr, 16);
        }
        check(
            "p",
            u32::MAX,
            &m_p,
            &[
                ("p", 0x0048, 0x00000001),
                ("p", 0x0094, 0xfffffffb),
                ("p", 0x0098, 0x00000001),
                ("p", 0x009c, 0xfffffffe),
            ],
        );
    }
}

#[test]
fn start_dma_ctr_1568() {
    {
        let mut m_p: Vec<u32> = vec![0; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::start_dma(&mut p, super::DmaBlockMode::Ctr, 1568);
        }
        check(
            "p",
            0,
            &m_p,
            &[
                ("p", 0x0048, 0x00000001),
                ("p", 0x0090, 0x00000001),
                ("p", 0x0094, 0x00000003),
                ("p", 0x0098, 0x00000062),
            ],
        );
    }
    {
        let mut m_p: Vec<u32> = vec![u32::MAX; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::start_dma(&mut p, super::DmaBlockMode::Ctr, 1568);
        }
        check(
            "p",
            u32::MAX,
            &m_p,
            &[
                ("p", 0x0048, 0x00000001),
                ("p", 0x0094, 0xfffffffb),
                ("p", 0x0098, 0x00000062),
                ("p", 0x009c, 0xfffffffe),
            ],
        );
    }
}

#[test]
fn start_dma_cfb8_16() {
    {
        let mut m_p: Vec<u32> = vec![0; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::start_dma(&mut p, super::DmaBlockMode::Cfb8, 16);
        }
        check(
            "p",
            0,
            &m_p,
            &[
                ("p", 0x0048, 0x00000001),
                ("p", 0x0090, 0x00000001),
                ("p", 0x0094, 0x00000004),
                ("p", 0x0098, 0x00000001),
            ],
        );
    }
    {
        let mut m_p: Vec<u32> = vec![u32::MAX; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::start_dma(&mut p, super::DmaBlockMode::Cfb8, 16);
        }
        check(
            "p",
            u32::MAX,
            &m_p,
            &[
                ("p", 0x0048, 0x00000001),
                ("p", 0x0094, 0xfffffffc),
                ("p", 0x0098, 0x00000001),
                ("p", 0x009c, 0xfffffffe),
            ],
        );
    }
}

#[test]
fn start_dma_cfb8_1568() {
    {
        let mut m_p: Vec<u32> = vec![0; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::start_dma(&mut p, super::DmaBlockMode::Cfb8, 1568);
        }
        check(
            "p",
            0,
            &m_p,
            &[
                ("p", 0x0048, 0x00000001),
                ("p", 0x0090, 0x00000001),
                ("p", 0x0094, 0x00000004),
                ("p", 0x0098, 0x00000062),
            ],
        );
    }
    {
        let mut m_p: Vec<u32> = vec![u32::MAX; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::start_dma(&mut p, super::DmaBlockMode::Cfb8, 1568);
        }
        check(
            "p",
            u32::MAX,
            &m_p,
            &[
                ("p", 0x0048, 0x00000001),
                ("p", 0x0094, 0xfffffffc),
                ("p", 0x0098, 0x00000062),
                ("p", 0x009c, 0xfffffffe),
            ],
        );
    }
}

#[test]
fn start_dma_cfb128_16() {
    {
        let mut m_p: Vec<u32> = vec![0; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::start_dma(&mut p, super::DmaBlockMode::Cfb128, 16);
        }
        check(
            "p",
            0,
            &m_p,
            &[
                ("p", 0x0048, 0x00000001),
                ("p", 0x0090, 0x00000001),
                ("p", 0x0094, 0x00000005),
                ("p", 0x0098, 0x00000001),
            ],
        );
    }
    {
        let mut m_p: Vec<u32> = vec![u32::MAX; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::start_dma(&mut p, super::DmaBlockMode::Cfb128, 16);
        }
        check(
            "p",
            u32::MAX,
            &m_p,
            &[
                ("p", 0x0048, 0x00000001),
                ("p", 0x0094, 0xfffffffd),
                ("p", 0x0098, 0x00000001),
                ("p", 0x009c, 0xfffffffe),
            ],
        );
    }
}

#[test]
fn start_dma_cfb128_1568() {
    {
        let mut m_p: Vec<u32> = vec![0; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::start_dma(&mut p, super::DmaBlockMode::Cfb128, 1568);
        }
        check(
            "p",
            0,
            &m_p,
            &[
                ("p", 0x0048, 0x00000001),
                ("p", 0x0090, 0x00000001),
                ("p", 0x0094, 0x00000005),
                ("p", 0x0098, 0x00000062),
            ],
        );
    }
    {
        let mut m_p: Vec<u32> = vec![u32::MAX; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::start_dma(&mut p, super::DmaBlockMode::Cfb128, 1568);
        }
        check(
            "p",
            u32::MAX,
            &m_p,
            &[
                ("p", 0x0048, 0x00000001),
                ("p", 0x0094, 0xfffffffd),
                ("p", 0x0098, 0x00000062),
                ("p", 0x009c, 0xfffffffe),
            ],
        );
    }
}

#[test]
fn start_dma_gcm_16() {
    {
        let mut m_p: Vec<u32> = vec![0; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::start_dma(&mut p, super::DmaBlockMode::Gcm, 16);
        }
        check(
            "p",
            0,
            &m_p,
            &[
                ("p", 0x0048, 0x00000001),
                ("p", 0x0090, 0x00000001),
                ("p", 0x0094, 0x00000006),
                ("p", 0x0098, 0x00000001),
            ],
        );
    }
    {
        let mut m_p: Vec<u32> = vec![u32::MAX; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::start_dma(&mut p, super::DmaBlockMode::Gcm, 16);
        }
        check(
            "p",
            u32::MAX,
            &m_p,
            &[
                ("p", 0x0048, 0x00000001),
                ("p", 0x0094, 0xfffffffe),
                ("p", 0x0098, 0x00000001),
                ("p", 0x009c, 0xfffffffe),
            ],
        );
    }
}

#[test]
fn start_dma_gcm_1568() {
    {
        let mut m_p: Vec<u32> = vec![0; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::start_dma(&mut p, super::DmaBlockMode::Gcm, 1568);
        }
        check(
            "p",
            0,
            &m_p,
            &[
                ("p", 0x0048, 0x00000001),
                ("p", 0x0090, 0x00000001),
                ("p", 0x0094, 0x00000006),
                ("p", 0x0098, 0x00000062),
            ],
        );
    }
    {
        let mut m_p: Vec<u32> = vec![u32::MAX; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesProgram::host(&mut m_p).expect("memory holds the part");
            ops::start_dma(&mut p, super::DmaBlockMode::Gcm, 1568);
        }
        check(
            "p",
            u32::MAX,
            &m_p,
            &[
                ("p", 0x0048, 0x00000001),
                ("p", 0x0094, 0xfffffffe),
                ("p", 0x0098, 0x00000062),
                ("p", 0x009c, 0xfffffffe),
            ],
        );
    }
}

#[test]
fn clear_interrupt() {
    {
        let mut m_p: Vec<u32> = vec![0; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesInterrupt::host(&mut m_p).expect("memory holds the part");
            ops::clear_interrupt(&mut p);
        }
        check("p", 0, &m_p, &[("p", 0x00ac, 0x00000001)]);
    }
    {
        let mut m_p: Vec<u32> = vec![u32::MAX; instances::aes::SIZE.div_ceil(4)];
        {
            let mut p = aes::AesInterrupt::host(&mut m_p).expect("memory holds the part");
            ops::clear_interrupt(&mut p);
        }
        check("p", u32::MAX, &m_p, &[("p", 0x00ac, 0x00000001)]);
    }
}

#[test]
fn enable_clock() {
    {
        let mut m_c: Vec<u32> = vec![0; instances::hp_sys_clkrst::SIZE.div_ceil(4)];
        {
            let mut c = hp_sys_clkrst::CryptoClock::host(&mut m_c).expect("memory holds the part");
            ops::enable_clock(&mut c);
        }
        check("c", 0, &m_c, &[("c", 0x0058, 0x00000010)]);
    }
    {
        let mut m_c: Vec<u32> = vec![u32::MAX; instances::hp_sys_clkrst::SIZE.div_ceil(4)];
        {
            let mut c = hp_sys_clkrst::CryptoClock::host(&mut m_c).expect("memory holds the part");
            ops::enable_clock(&mut c);
        }
        check("c", u32::MAX, &m_c, &[]);
    }
}

#[test]
fn disable_clock() {
    {
        let mut m_c: Vec<u32> = vec![0; instances::hp_sys_clkrst::SIZE.div_ceil(4)];
        {
            let mut c = hp_sys_clkrst::CryptoClock::host(&mut m_c).expect("memory holds the part");
            ops::disable_clock(&mut c);
        }
        check("c", 0, &m_c, &[]);
    }
    {
        let mut m_c: Vec<u32> = vec![u32::MAX; instances::hp_sys_clkrst::SIZE.div_ceil(4)];
        {
            let mut c = hp_sys_clkrst::CryptoClock::host(&mut m_c).expect("memory holds the part");
            ops::disable_clock(&mut c);
        }
        check("c", u32::MAX, &m_c, &[("c", 0x0058, 0xffffffef)]);
    }
}

#[test]
fn assert_reset() {
    {
        let mut m_c: Vec<u32> = vec![0; instances::hp_sys_clkrst::SIZE.div_ceil(4)];
        {
            let mut c = hp_sys_clkrst::CryptoClock::host(&mut m_c).expect("memory holds the part");
            ops::assert_reset(&mut c);
        }
        check("c", 0, &m_c, &[("c", 0x0058, 0x00000020)]);
    }
    {
        let mut m_c: Vec<u32> = vec![u32::MAX; instances::hp_sys_clkrst::SIZE.div_ceil(4)];
        {
            let mut c = hp_sys_clkrst::CryptoClock::host(&mut m_c).expect("memory holds the part");
            ops::assert_reset(&mut c);
        }
        check("c", u32::MAX, &m_c, &[]);
    }
}

#[test]
fn release_reset() {
    {
        let mut m_c: Vec<u32> = vec![0; instances::hp_sys_clkrst::SIZE.div_ceil(4)];
        {
            let mut c = hp_sys_clkrst::CryptoClock::host(&mut m_c).expect("memory holds the part");
            ops::release_reset(&mut c);
        }
        check("c", 0, &m_c, &[]);
    }
    {
        let mut m_c: Vec<u32> = vec![u32::MAX; instances::hp_sys_clkrst::SIZE.div_ceil(4)];
        {
            let mut c = hp_sys_clkrst::CryptoClock::host(&mut m_c).expect("memory holds the part");
            ops::release_reset(&mut c);
        }
        check("c", u32::MAX, &m_c, &[("c", 0x0058, 0xffffffdf)]);
    }
}

#[test]
fn release_parent_resets() {
    {
        let mut m_c: Vec<u32> = vec![0; instances::hp_sys_clkrst::SIZE.div_ceil(4)];
        {
            let mut c = hp_sys_clkrst::CryptoClock::host(&mut m_c).expect("memory holds the part");
            ops::release_parent_resets(&mut c);
        }
        check("c", 0, &m_c, &[]);
    }
    {
        let mut m_c: Vec<u32> = vec![u32::MAX; instances::hp_sys_clkrst::SIZE.div_ceil(4)];
        {
            let mut c = hp_sys_clkrst::CryptoClock::host(&mut m_c).expect("memory holds the part");
            ops::release_parent_resets(&mut c);
        }
        check("c", u32::MAX, &m_c, &[("c", 0x0058, 0xfffff7fd)]);
    }
}

#[test]
fn dma_prepare() {
    {
        let mut m_c: Vec<u32> = vec![0; instances::hp_sys_clkrst::SIZE.div_ceil(4)];
        let mut m_d: Vec<u32> = vec![0; instances::axi_dma::SIZE.div_ceil(4)];
        {
            let mut c = hp_sys_clkrst::AxiDmaClock::host(&mut m_c).expect("memory holds the part");
            let mut d = axi_dma::AxiDmaChannel0::host(&mut m_d).expect("memory holds the part");
            crate::peripherals::aes_dma::prepare(&mut c, &mut d);
        }
        check(
            "c",
            0,
            &m_c,
            &[
                ("c", 0x0078, 0x00000001),
                ("d", 0x0044, 0x00000004),
                ("d", 0x017c, 0x00000004),
                ("d", 0x027c, 0x2f000000),
                ("d", 0x0280, 0x2f07ffff),
                ("d", 0x0284, 0x40000000),
                ("d", 0x0288, 0x53ffffff),
                ("d", 0x02a8, 0x00000010),
            ],
        );
        check(
            "d",
            0,
            &m_d,
            &[
                ("c", 0x0078, 0x00000001),
                ("d", 0x0044, 0x00000004),
                ("d", 0x017c, 0x00000004),
                ("d", 0x027c, 0x2f000000),
                ("d", 0x0280, 0x2f07ffff),
                ("d", 0x0284, 0x40000000),
                ("d", 0x0288, 0x53ffffff),
                ("d", 0x02a8, 0x00000010),
            ],
        );
    }
    {
        let mut m_c: Vec<u32> = vec![u32::MAX; instances::hp_sys_clkrst::SIZE.div_ceil(4)];
        let mut m_d: Vec<u32> = vec![u32::MAX; instances::axi_dma::SIZE.div_ceil(4)];
        {
            let mut c = hp_sys_clkrst::AxiDmaClock::host(&mut m_c).expect("memory holds the part");
            let mut d = axi_dma::AxiDmaChannel0::host(&mut m_d).expect("memory holds the part");
            crate::peripherals::aes_dma::prepare(&mut c, &mut d);
        }
        check(
            "c",
            u32::MAX,
            &m_c,
            &[
                ("c", 0x0078, 0xfffffffd),
                ("d", 0x0010, 0xfffffffb),
                ("d", 0x0044, 0xffffffc4),
                ("d", 0x017c, 0xffffffc4),
                ("d", 0x027c, 0x2f000000),
                ("d", 0x0280, 0x2f07ffff),
                ("d", 0x0284, 0x40000000),
                ("d", 0x0288, 0x53ffffff),
            ],
        );
        check(
            "d",
            u32::MAX,
            &m_d,
            &[
                ("c", 0x0078, 0xfffffffd),
                ("d", 0x0010, 0xfffffffb),
                ("d", 0x0044, 0xffffffc4),
                ("d", 0x017c, 0xffffffc4),
                ("d", 0x027c, 0x2f000000),
                ("d", 0x0280, 0x2f07ffff),
                ("d", 0x0284, 0x40000000),
                ("d", 0x0288, 0x53ffffff),
            ],
        );
    }
}

/// `is_done` answers `DONE` alone of the field's four codes.
#[test]
fn only_the_done_code_is_done() {
    for code in 0..4_u32 {
        let mut memory: Vec<u32> = vec![0; instances::aes::SIZE.div_ceil(4)];
        memory[aes::state::OFFSET / 4] = code << aes::state::STATE_MASK.trailing_zeros();
        let program = aes::AesProgram::host(&mut memory).expect("memory holds the part");
        assert_eq!(
            ops::is_done(&program),
            aes::state::State::from_bits(code) == Some(aes::state::State::Done),
            "code {code}"
        );
    }
}
