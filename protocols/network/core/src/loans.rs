//! What receive loans may hold of a driver's buffers.
//!
//! A socket in the lending mode holds frames in the radio's own buffers, up to
//! its credit, for as long as its application likes. A credit bounds memory and
//! nothing else: it limits neither the work per turn nor the air. What is
//! granted, the radio's reserve and the loans of earlier installations together
//! fit the pool; [`Loans::grant`] refuses more. That is needed for a slow
//! consumer to leave the others their buffers, and not sufficient for it.
//!
//! A loan held for ever hangs nothing: a stop refuses new loans at once and
//! says what is still out. Its memory stays with its holder until it comes
//! back, and no expiry frees it.

/// Why [`Loans`] cannot be, or a credit cannot be granted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoansRefused {
    /// The reserve does not fit the pool.
    Reserve,
    /// No such socket.
    Socket,
    /// A stop refuses new credit until the next installation.
    Stopping,
    /// The credits, the reserve and old loans would pass the pool: `room` fits.
    Pool { room: usize },
}

/// Why a frame was not lent. Its buffer goes back to the radio at once.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NotLent {
    /// The socket holds all its credit.
    NoCredit,
    /// No such socket.
    Socket,
    /// The installation is stopping.
    Stopping,
}

/// The right to one lent buffer. It is not `Copy`, so a buffer comes back once.
#[derive(Debug, Eq, PartialEq)]
pub struct Loan {
    socket: usize,
    installation: u32,
}

/// The loans of a pool of driver buffers, by socket.
#[derive(Debug)]
pub struct Loans<const SOCKETS: usize> {
    buffers: usize,
    reserve: usize,
    credits: [usize; SOCKETS],
    held: [usize; SOCKETS],
    /// Loans of earlier installations still out.
    old: usize,
    installation: u32,
    stopping: bool,
}

impl<const SOCKETS: usize> Loans<SOCKETS> {
    /// A pool of `buffers`, of which loans never reach `reserve`: the radio's
    /// ring and what needs no loan live on those.
    ///
    /// # Errors
    /// [`LoansRefused::Reserve`].
    pub const fn new(buffers: usize, reserve: usize) -> Result<Self, LoansRefused> {
        if reserve > buffers {
            return Err(LoansRefused::Reserve);
        }
        Ok(Self {
            buffers,
            reserve,
            credits: [0; SOCKETS],
            held: [0; SOCKETS],
            old: 0,
            installation: 0,
            stopping: false,
        })
    }

    /// Give `socket` a credit of `credit`. A smaller credit than it holds
    /// takes nothing back: it lends again once it returned enough.
    ///
    /// # Errors
    /// [`LoansRefused`]; the old credit stays.
    pub fn grant(&mut self, socket: usize, credit: usize) -> Result<(), LoansRefused> {
        if self.stopping {
            return Err(LoansRefused::Stopping);
        }
        let others = self
            .credits
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != socket)
            .fold(0_usize, |sum, (_, granted)| sum.saturating_add(*granted));
        let taken = others.saturating_add(self.reserve).saturating_add(self.old);
        let room = self.buffers.saturating_sub(taken);
        let place = self.credits.get_mut(socket).ok_or(LoansRefused::Socket)?;
        if credit > room {
            return Err(LoansRefused::Pool { room });
        }
        *place = credit;
        Ok(())
    }

    /// A frame for `socket` arrived: lend it, or say why not, before any more
    /// work goes into it.
    ///
    /// # Errors
    /// [`NotLent`]; nothing changed.
    pub fn lend(&mut self, socket: usize) -> Result<Loan, NotLent> {
        if self.stopping {
            return Err(NotLent::Stopping);
        }
        let credit = *self.credits.get(socket).ok_or(NotLent::Socket)?;
        let held = self.held.get_mut(socket).ok_or(NotLent::Socket)?;
        if *held >= credit {
            return Err(NotLent::NoCredit);
        }
        *held = held.saturating_add(1);
        Ok(Loan {
            socket,
            installation: self.installation,
        })
    }

    /// The buffer of `loan` is back with the radio's pool.
    pub fn give_back(&mut self, loan: Loan) {
        if loan.installation != self.installation {
            self.old = self.old.saturating_sub(1);
        } else if let Some(held) = self.held.get_mut(loan.socket) {
            *held = held.saturating_sub(1);
        }
    }

    /// Stop lending. The hardware's stop waits for nothing here.
    pub const fn stop(&mut self) {
        self.stopping = true;
    }

    /// Loans out, old installations' included.
    pub fn outstanding(&self) -> usize {
        self.held
            .iter()
            .fold(self.old, |sum, held| sum.saturating_add(*held))
    }

    /// Whether every buffer came back, so nothing keeps the pool from reuse.
    pub fn released(&self) -> bool {
        self.outstanding() == 0
    }

    /// Begin the next installation over the same pool: credits start at zero,
    /// and what is still out counts against them until it comes back.
    pub fn reinstall(&mut self) {
        self.old = self.outstanding();
        self.held = [0; SOCKETS];
        self.credits = [0; SOCKETS];
        self.installation = self.installation.wrapping_add(1);
        self.stopping = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credits_and_the_reserve_fit_the_pool() {
        let mut loans = Loans::<2>::new(10, 4).unwrap();
        loans.grant(0, 4).unwrap();
        assert_eq!(loans.grant(1, 3), Err(LoansRefused::Pool { room: 2 }));
        loans.grant(1, 2).unwrap();
        assert_eq!(loans.grant(2, 1), Err(LoansRefused::Socket));
    }

    #[test]
    fn a_reserve_past_the_pool_is_refused() {
        assert_eq!(Loans::<1>::new(3, 4).err(), Some(LoansRefused::Reserve));
    }

    #[test]
    fn a_socket_lends_up_to_its_credit_and_again_after_a_return() {
        let mut loans = Loans::<1>::new(4, 2).unwrap();
        loans.grant(0, 1).unwrap();
        let loan = loans.lend(0).unwrap();
        assert_eq!(loans.lend(0), Err(NotLent::NoCredit));
        loans.give_back(loan);
        assert!(loans.lend(0).is_ok());
    }

    #[test]
    fn a_smaller_credit_takes_nothing_back() {
        let mut loans = Loans::<1>::new(4, 0).unwrap();
        loans.grant(0, 2).unwrap();
        let first = loans.lend(0).unwrap();
        let second = loans.lend(0).unwrap();
        loans.grant(0, 1).unwrap();
        assert_eq!(loans.outstanding(), 2);
        loans.give_back(first);
        assert_eq!(loans.lend(0), Err(NotLent::NoCredit));
        loans.give_back(second);
        assert!(loans.lend(0).is_ok());
    }

    #[test]
    fn a_stop_refuses_new_loans_and_keeps_what_is_out() {
        let mut loans = Loans::<1>::new(4, 0).unwrap();
        loans.grant(0, 2).unwrap();
        let held = loans.lend(0).unwrap();
        loans.stop();
        assert_eq!(loans.lend(0), Err(NotLent::Stopping));
        assert_eq!(loans.grant(0, 1), Err(LoansRefused::Stopping));
        assert!(!loans.released());
        loans.give_back(held);
        assert!(loans.released());
    }

    #[test]
    fn an_old_installations_loan_counts_until_it_comes_back() {
        let mut loans = Loans::<1>::new(4, 1).unwrap();
        loans.grant(0, 3).unwrap();
        let old = loans.lend(0).unwrap();
        let older = loans.lend(0).unwrap();
        loans.stop();
        loans.reinstall();
        assert_eq!(loans.grant(0, 2), Err(LoansRefused::Pool { room: 1 }));
        loans.grant(0, 1).unwrap();
        let new = loans.lend(0).unwrap();
        loans.give_back(old);
        assert_eq!(loans.outstanding(), 2);
        loans.give_back(older);
        loans.give_back(new);
        assert!(loans.released());
    }
}
