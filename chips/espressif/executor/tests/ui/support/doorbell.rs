// The actual doorbell implementation with a synthetic host-owned binding and
// software lines. No chip instructions, MMIO or unsafe authority are needed
// for this UI case.
mod software_line {
    pub trait SoftwareLine {
        fn raise(&self);
        fn lower(&self);
    }
}
mod chip {
    use core::marker::PhantomData;
    pub trait Port {
        type Line0;
        type Line1;
        type Line2;
        type Line3;
    }
    pub trait Binding {
        type Port: Port;
    }
    pub type PortOf<B> = <B as Binding>::Port;
    pub type Line0<B> = <PortOf<B> as Port>::Line0;
    pub type Line1<B> = <PortOf<B> as Port>::Line1;
    pub type Line2<B> = <PortOf<B> as Port>::Line2;
    pub type Line3<B> = <PortOf<B> as Port>::Line3;
    pub struct Line<P>(PhantomData<P>);
    impl<P> Clone for Line<P> {
        fn clone(&self) -> Self {
            *self
        }
    }
    impl<P> Copy for Line<P> {}
    impl<P> super::software_line::SoftwareLine for Line<P> {
        fn raise(&self) {}
        fn lower(&self) {}
    }
    pub const fn software_interrupt_source<P>(swi: u8) -> u16 {
        swi as u16
    }
    pub mod platform {
        pub struct FromCpu0;
        pub struct FromCpu1;
        pub struct FromCpu2;
        pub struct FromCpu3;
    }
    pub struct FakePort;
    impl Port for FakePort {
        type Line0 = platform::FromCpu0;
        type Line1 = platform::FromCpu1;
        type Line2 = platform::FromCpu2;
        type Line3 = platform::FromCpu3;
    }
    pub struct Fake;
    impl Binding for Fake {
        type Port = FakePort;
    }
    pub fn own_line_0<B: Binding>(_: Line0<B>) -> Line<PortOf<B>> {
        Line(PhantomData)
    }
    pub fn own_line_1<B: Binding>(_: Line1<B>) -> Line<PortOf<B>> {
        Line(PhantomData)
    }
    pub fn own_line_2<B: Binding>(_: Line2<B>) -> Line<PortOf<B>> {
        Line(PhantomData)
    }
    pub fn own_line_3<B: Binding>(_: Line3<B>) -> Line<PortOf<B>> {
        Line(PhantomData)
    }
}
#[path = "../../../src/doorbell.rs"]
mod doorbell;
#[path = "../../../src/urgent_sources.rs"]
mod urgent_sources;
