use super::*;
use crate::{Slot, Slots, StaticId};

enum Declaration {}
enum Other {}
enum Tokens {}

type Layout = Slots<Tokens, 2>;

fn claim<D>() -> SlotClaim<StaticId<D>> {
    SlotClaim {
        identity: PhantomData,
    }
}

#[test]
fn the_first_installation_is_minted_once_with_its_whole_pool_set() {
    let mut installations = Installations::<StaticId<Declaration>, Layout>::claim(claim());
    let Some(Minted { mint, installation }) = installations.first() else {
        panic!("the first installation is minted");
    };
    let [first, second]: [Slot<'static, Tokens, _>; 2] = mint.into_pools();
    assert_eq!((first.index(), second.index()), (0, 1));
    assert_eq!(installation.generation(), 0);
    assert!(
        installations.first().is_none(),
        "the first mint is not repeated"
    );
    assert_eq!(installations.generation(), 0);
}

#[test]
fn only_the_current_receipt_mints_the_next_installation() {
    let mut installations = Installations::<StaticId<Declaration>, Layout>::claim(claim());
    let first = installations.first().expect("first").installation;
    let second = installations
        .remint(first)
        .expect("the current receipt re-mints");
    assert_eq!(second.installation.generation(), 1);
    assert_eq!(installations.generation(), 1);
    let third = installations
        .remint(second.installation)
        .expect("and again with the next one");
    assert_eq!(third.installation.generation(), 2);
}

#[test]
fn a_receipt_of_another_installation_is_refused_and_returned_unchanged() {
    // Two authorities of one identity exist only inside this crate's tests:
    // a static claim is made once per declaration.
    let mut current = Installations::<StaticId<Declaration>, Layout>::claim(claim());
    let mut stale = Installations::<StaticId<Declaration>, Layout>::claim(claim());
    let live = current.first().expect("first").installation;
    let live = current.remint(live).expect("current").installation;
    let foreign = stale.first().expect("first").installation;
    match current.remint(foreign) {
        Err(RemintRefused::Foreign(receipt)) => assert_eq!(receipt.generation(), 0),
        _ => panic!("a stale receipt mints nothing"),
    }
    assert_eq!(current.generation(), 1, "the refusal changed nothing");
    assert!(current.remint(live).is_ok(), "the live receipt still mints");
}

#[test]
fn a_receipt_before_the_first_installation_is_foreign() {
    let mut other = Installations::<StaticId<Other>, Layout>::claim(claim());
    let receipt = other.first().expect("first").installation;
    let mut unissued = Installations::<StaticId<Other>, Layout>::claim(claim());
    assert!(matches!(
        unissued.remint(receipt),
        Err(RemintRefused::Foreign(_))
    ));
    assert!(unissued.first().is_some(), "the refusal issued nothing");
}

#[test]
fn exhausted_generations_refuse_with_the_receipt() {
    let mut installations = Installations::<StaticId<Declaration>, Layout>::claim(claim());
    let mut receipt = installations.first().expect("first").installation;
    installations.generation = u32::MAX;
    receipt.generation = u32::MAX;
    match installations.remint(receipt) {
        Err(RemintRefused::Exhausted(receipt)) => assert_eq!(receipt.generation(), u32::MAX),
        _ => panic!("no generation is left"),
    }
    assert_eq!(installations.generation(), u32::MAX);
}

#[test]
fn a_local_identity_lends_its_owned_tokens_within_its_scope() {
    use crate::LocalId;
    struct Owner;
    impl<'scope> SlotOwner<LocalId<'scope>> for Owner {
        type Pools = Layout;
        type Queues = ();
        const VACANT_QUEUES: Self::Queues = ();
        type Vacant = ();
        type Frontend<'s>
            = ()
        where
            Self: 's,
            LocalId<'scope>: 's;
        type Invocation<'s>
            = ()
        where
            Self: 's,
            LocalId<'scope>: 's;
        fn prepare_queues(
            &mut self,
            _: &mut Self::Queues,
            _: Mint<'static, Self::Pools, LocalId<'scope>>,
        ) {
        }
        fn split<'s>(
            _: &'s mut Self::Queues,
            _: crate::SlotWakes<'s, LocalId<'scope>>,
        ) -> Result<(Self::Frontend<'s>, Self::Invocation<'s>), crate::SlotInitError>
        where
            Self: 's,
            LocalId<'scope>: 's,
        {
            Ok(((), ()))
        }
        async fn run<'s>(
            &mut self,
            _: &mut Self::Invocation<'s>,
            _: crate::SlotRetire<'_, LocalId<'scope>, Self>,
        ) -> crate::SlotStopped<LocalId<'scope>, Self>
        where
            Self: 's,
            LocalId<'scope>: 's,
        {
            core::future::pending().await
        }
        fn finish<'s>(&mut self, _: &mut Self::Invocation<'s>) -> Self::Vacant
        where
            Self: 's,
            LocalId<'scope>: 's,
        {
        }
    }
    fn lent<'scope>(claim: SlotClaim<LocalId<'scope>>) -> [u8; 2] {
        let mut installations = Installations::<LocalId<'scope>, Layout>::claim(claim);
        let Minted { mint, installation } = installations.first().expect("first");
        let loan: BodyLoan<'scope, _, Owner> = installation.loan();
        let [first, second] = mint.into_pools();
        let first: Slot<'scope, Tokens, _> = loan.lend_slot(first);
        let second: Slot<'static, Tokens, _> = loan.reclaim_slot(loan.lend_slot(second));
        [first.index(), second.index()]
    }
    let indices = lent(SlotClaim::<LocalId<'_>> {
        identity: PhantomData,
    });
    assert_eq!(indices, [0, 1], "the conversion keeps each token's index");
}
