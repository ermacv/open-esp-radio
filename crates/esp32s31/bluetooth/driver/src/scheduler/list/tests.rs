use std::vec::Vec;

use super::{
    SchedulerList, SchedulerListCompletionScan, SchedulerListInsertError, SchedulerListPlacement,
    SchedulerListRemoval,
};
use crate::scheduler::window::SchedulerRawWindow;

fn window(start: u32, end: u32) -> SchedulerRawWindow {
    SchedulerRawWindow::new(start, end).expect("test window is valid")
}

fn order<const CAPACITY: usize>(list: &SchedulerList<u8, CAPACITY>) -> Vec<u8> {
    list.iter().map(|(id, _)| id).collect()
}

#[test]
fn events_are_kept_in_start_order_with_their_neighbours() {
    let mut list = SchedulerList::<u8, 4>::new();
    assert_eq!(
        list.insert(1, window(100, 200)),
        Ok(SchedulerListPlacement {
            predecessor: None,
            successor: None
        })
    );
    assert_eq!(
        list.insert(2, window(300, 400)),
        Ok(SchedulerListPlacement {
            predecessor: Some(1),
            successor: None
        })
    );
    let placement = list
        .insert(3, window(200, 300))
        .expect("touching windows fit");
    assert_eq!(
        placement,
        SchedulerListPlacement {
            predecessor: Some(1),
            successor: Some(2)
        }
    );
    let head = list
        .insert(4, window(10, 100))
        .expect("an earlier event becomes the head");
    assert!(head.is_head());
    assert_eq!(head.successor, Some(1));

    assert_eq!(order(&list), [4, 1, 3, 2]);
    assert_eq!(list.head(), Some((4, window(10, 100))));
    assert_eq!(list.window(3), Some(window(200, 300)));
}

#[test]
fn planning_does_not_change_the_list() {
    let mut list = SchedulerList::<u8, 2>::new();
    list.insert(1, window(100, 200)).unwrap();
    assert_eq!(
        list.plan_insert(2, window(0, 50)),
        Ok(SchedulerListPlacement {
            predecessor: None,
            successor: Some(1)
        })
    );
    assert_eq!(order(&list), [1]);
}

#[test]
fn rejections_leave_the_list_unchanged() {
    let mut list = SchedulerList::<u8, 2>::new();
    list.insert(1, window(100, 200)).unwrap();

    assert_eq!(
        list.insert(1, window(300, 400)),
        Err(SchedulerListInsertError::Duplicate)
    );
    assert_eq!(
        list.insert(2, window(150, 250)),
        Err(SchedulerListInsertError::Overlap { with: 1 })
    );
    assert_eq!(
        list.insert(2, window(50, 101)),
        Err(SchedulerListInsertError::Overlap { with: 1 })
    );
    list.insert(2, window(300, 400)).unwrap();
    assert_eq!(
        list.insert(3, window(500, 600)),
        Err(SchedulerListInsertError::Full)
    );
    assert_eq!(order(&list), [1, 2]);
}

#[test]
fn ordering_follows_wrapping_controller_time() {
    let mut list = SchedulerList::<u8, 3>::new();
    list.insert(1, window(0xffff_ff00, 0xffff_ff80)).unwrap();
    list.insert(2, window(0x0000_0010, 0x0000_0020)).unwrap();
    list.insert(3, window(0xffff_ffc0, 0x0000_0008)).unwrap();
    assert_eq!(order(&list), [1, 3, 2]);
    assert_eq!(
        list.insert(4, window(0xffff_fff0, 0x0000_0010)),
        Err(SchedulerListInsertError::Full)
    );
}

#[test]
fn the_list_spans_at_most_one_forward_half_range() {
    let mut list = SchedulerList::<u8, 3>::new();
    list.insert(1, window(0, 10)).unwrap();
    list.insert(2, window(0x7000_0000, 0x7000_0010)).unwrap();
    // After the head but too far from it.
    assert_eq!(
        list.insert(3, window(0x8000_0010, 0x8000_0020)),
        Err(SchedulerListInsertError::OutsideForwardRange)
    );
    // Before the head, which would stretch the list past the tail.
    assert_eq!(
        list.insert(3, window(0xf000_0000, 0xf000_0010)),
        Err(SchedulerListInsertError::OutsideForwardRange)
    );
    list.insert(3, window(0xffff_ff00, 0xffff_ff10)).unwrap();
    assert_eq!(order(&list), [3, 1, 2]);
}

#[test]
fn removal_reports_the_neighbours_to_relink() {
    let mut list = SchedulerList::<u8, 4>::new();
    for (id, start) in [(1, 0), (2, 100), (3, 200)] {
        list.insert(id, window(start, start + 50)).unwrap();
    }
    assert_eq!(
        list.remove(2),
        Some(SchedulerListRemoval {
            predecessor: Some(1),
            successor: Some(3),
            window: window(100, 150)
        })
    );
    assert_eq!(
        list.remove(1),
        Some(SchedulerListRemoval {
            predecessor: None,
            successor: Some(3),
            window: window(0, 50)
        })
    );
    assert_eq!(list.remove(1), None);
    assert_eq!(order(&list), [3]);
    list.insert(4, window(100, 150)).unwrap();
    assert_eq!(order(&list), [4, 3]);
    assert_eq!(list.len(), 2);
}

#[test]
fn completion_scan_follows_the_vendor_walk() {
    let mut list = SchedulerList::<u8, 5>::new();
    for id in 1..=5 {
        let start = u32::from(id) * 100;
        list.insert(id, window(start, start + 50)).unwrap();
    }

    assert_eq!(
        list.scan_completion(|id| id <= 2),
        SchedulerListCompletionScan {
            completed_prefix: 2,
            out_of_order: false
        }
    );
    // One unexecuted event may be passed; an executed one behind it is
    // reported as out of order.
    assert_eq!(
        list.scan_completion(|id| id != 2),
        SchedulerListCompletionScan {
            completed_prefix: 1,
            out_of_order: true
        }
    );
    // The walk stops at the second unexecuted event.
    assert_eq!(
        list.scan_completion(|id| id == 1 || id == 4),
        SchedulerListCompletionScan {
            completed_prefix: 1,
            out_of_order: false
        }
    );
    assert_eq!(
        list.scan_completion(|_| false),
        SchedulerListCompletionScan {
            completed_prefix: 0,
            out_of_order: false
        }
    );
}
