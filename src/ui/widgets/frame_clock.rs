// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::meter::MeterEngine;
use crate::persistence::settings::VisualFrameRate;
use iced::advanced::widget::Tree;
use iced::advanced::{Clipboard, Layout, Shell, Widget, layout, mouse};
use iced::{Element, Event, Length, Rectangle, Size, Theme, window};
use std::cell::RefCell;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

const WATCHDOG_INTERVAL: Duration = Duration::from_millis(50);

fn next_deadline(deadline: Instant, now: Instant, interval: Duration) -> Instant {
    let next = deadline + interval;
    if next > now { next } else { now + interval }
}

fn display_frame_due(
    owner: Option<(window::Id, Instant)>,
    window: window::Id,
    is_main: bool,
    now: Instant,
) -> bool {
    is_main
        || owner.is_none_or(|(id, frame)| {
            window == id || now.saturating_duration_since(frame) >= WATCHDOG_INTERVAL
        })
}

#[derive(Clone, Default)]
pub(in crate::ui) struct FrameHeartbeat(Arc<AtomicU64>);

impl FrameHeartbeat {
    fn mark(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    fn generation(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }

    fn is_current(&self, generation: u64) -> bool {
        self.generation() == generation
    }
}

impl Hash for FrameHeartbeat {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.0).hash(state);
    }
}

pub(in crate::ui) fn frame_watchdog(heartbeat: &FrameHeartbeat) -> async_channel::Receiver<u64> {
    let (sender, receiver) = async_channel::bounded(1);
    let heartbeat = heartbeat.clone();
    if let Err(err) = std::thread::Builder::new()
        .name("openmeters-ui-watchdog".into())
        .spawn(move || {
            let mut generation = heartbeat.generation();
            loop {
                std::thread::sleep(WATCHDOG_INTERVAL);
                if sender.is_closed() {
                    break;
                }
                let current = heartbeat.generation();
                if current == generation {
                    match sender.try_send(current) {
                        Ok(()) | Err(async_channel::TrySendError::Full(_)) => {}
                        Err(async_channel::TrySendError::Closed(_)) => break,
                    }
                }
                generation = current;
            }
        })
    {
        tracing::error!("[ui] failed to start frame watchdog: {err}");
    }
    receiver
}

pub(in crate::ui) struct FrameCoordinator {
    meter: MeterEngine,
    rate: VisualFrameRate,
    owner: Option<(window::Id, Instant)>,
    next_frame: Option<Instant>,
    heartbeat: FrameHeartbeat,
}

impl FrameCoordinator {
    pub(in crate::ui) fn new(meter: MeterEngine, rate: VisualFrameRate) -> Self {
        Self {
            meter,
            rate,
            owner: None,
            next_frame: None,
            heartbeat: FrameHeartbeat::default(),
        }
    }

    fn frame(&mut self, window: window::Id, is_main: bool, now: Instant) -> Option<Instant> {
        self.heartbeat.mark();
        let Some(interval) = self.rate.interval() else {
            if display_frame_due(self.owner, window, is_main, now) {
                self.owner = Some((window, now));
                self.meter.advance(now);
            }
            return None;
        };

        let deadline = self.next_frame.unwrap_or(now);
        if now >= deadline {
            self.meter.advance(now);
            self.next_frame = Some(next_deadline(deadline, now, interval));
        }
        self.next_frame
    }

    pub(in crate::ui) fn heartbeat_handle(&self) -> FrameHeartbeat {
        self.heartbeat.clone()
    }

    pub(in crate::ui) fn watchdog(&mut self, generation: u64, now: Instant) {
        if self.heartbeat.is_current(generation) {
            self.meter.advance(now);
            self.next_frame = self.rate.interval().map(|interval| now + interval);
        }
    }

    fn reset_clock(&mut self) {
        self.owner = None;
        self.next_frame = None;
        self.heartbeat.mark();
    }

    pub(in crate::ui) fn set_rate(&mut self, rate: VisualFrameRate) {
        self.rate = rate;
        self.reset_clock();
    }

    pub(in crate::ui) fn set_active(&mut self, active: bool) {
        self.meter.set_active(active);
        self.reset_clock();
    }

    pub(in crate::ui) fn set_paused(&mut self, paused: bool, now: Instant) {
        self.meter.set_paused(paused, now);
        self.reset_clock();
    }
}

pub(in crate::ui) fn frame_clock<Message: 'static>(
    coordinator: Rc<RefCell<FrameCoordinator>>,
    window: window::Id,
    is_main: bool,
) -> Element<'static, Message> {
    Element::new(FrameClock {
        coordinator,
        window,
        is_main,
    })
}

struct FrameClock {
    coordinator: Rc<RefCell<FrameCoordinator>>,
    window: window::Id,
    is_main: bool,
}

impl<Message> Widget<Message, Theme, iced::Renderer> for FrameClock {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn layout(
        &mut self,
        _: &mut Tree,
        _: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(limits.max())
    }

    fn update(
        &mut self,
        _: &mut Tree,
        event: &Event,
        _: Layout<'_>,
        _: mouse::Cursor,
        _: &iced::Renderer,
        _: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _: &Rectangle,
    ) {
        if let Event::Window(window::Event::RedrawRequested(now)) = event {
            match self
                .coordinator
                .borrow_mut()
                .frame(self.window, self.is_main, *now)
            {
                Some(deadline) => shell.request_redraw_at(deadline),
                None => shell.request_redraw(),
            }
        }
    }

    fn draw(
        &self,
        _: &Tree,
        _: &mut iced::Renderer,
        _: &Theme,
        _: &iced::advanced::renderer::Style,
        _: Layout<'_>,
        _: mouse::Cursor,
        _: &Rectangle,
    ) {
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_deadlines_preserve_phase_and_drop_missed_frames() {
        let start = Instant::now();
        let interval = Duration::from_millis(10);
        let deadline = start + interval;
        assert_eq!(
            next_deadline(deadline, start + Duration::from_millis(12), interval),
            start + Duration::from_millis(20)
        );
        let late = start + Duration::from_millis(21);
        assert_eq!(next_deadline(deadline, late, interval), late + interval);
    }

    #[test]
    fn main_window_owns_display_cadence_with_popout_failover() {
        let main = window::Id::unique();
        let popout = window::Id::unique();
        let start = Instant::now();
        assert!(!display_frame_due(
            Some((main, start)),
            popout,
            false,
            start + WATCHDOG_INTERVAL / 2
        ));
        assert!(display_frame_due(
            Some((popout, start)),
            main,
            true,
            start + Duration::from_millis(1)
        ));
        assert!(display_frame_due(
            Some((main, start)),
            popout,
            false,
            start + WATCHDOG_INTERVAL
        ));
    }

    #[test]
    fn presentation_invalidates_queued_watchdog_ticks() {
        let heartbeat = FrameHeartbeat::default();
        let stale = heartbeat.generation();
        assert!(heartbeat.is_current(stale));
        heartbeat.mark();
        assert!(!heartbeat.is_current(stale));
    }
}
