// The actual doorbell implementation with synthetic host-owned software lines.
// No chip instructions, MMIO or unsafe authority are needed for this UI case.
mod software_line {
    pub trait SoftwareLine {
        fn raise(&self);
        fn lower(&self);
    }
}
mod chip {
    #[derive(Clone, Copy)]
    pub struct Line;
    impl super::software_line::SoftwareLine for Line {
        fn raise(&self) {}
        fn lower(&self) {}
    }
    pub const fn software_interrupt_source(swi: u8) -> u16 { swi as u16 }
    pub mod platform {
        pub struct FromCpu0;
        pub struct FromCpu1;
        pub struct FromCpu2;
        pub struct FromCpu3;
    }
    pub fn own_line_0(_: platform::FromCpu0) -> Line { Line }
    pub fn own_line_1(_: platform::FromCpu1) -> Line { Line }
    pub fn own_line_2(_: platform::FromCpu2) -> Line { Line }
    pub fn own_line_3(_: platform::FromCpu3) -> Line { Line }
}
#[path = "../../../../../chips/espressif/executor/src/doorbell.rs"]
mod doorbell;
