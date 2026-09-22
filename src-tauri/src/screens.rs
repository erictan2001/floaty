//! The monitors, as floaty models them: two spaces, and a mapping between them.
//!
//! - **Physical** is what the platform reports and what a window is placed in. On a
//!   mixed-DPI desktop it is the only space that is continuous.
//! - **Virtual** is the space a record's x/y are stored in, and the space a page's
//!   css px are in when the page is an overlay on a screen. Each screen contributes
//!   `physical / scale` of its own, and Windows reports a screen's *origin* in
//!   physical px — so two screens at different scales do **not** tile. Measured pair:
//!   2880x1920 @2 and 1920x1080 @1.5 give the second screen a virtual origin of 1920
//!   while the first ends at 1440, a 480px band that belongs to no screen at all.
//!   Nothing is ever stored there (`rehome_stranded` moves out of it) and the mapping
//!   is piecewise, so the band never has to mean anything.
//!
//! A **pointer** is mapped through the screen it is physically on: that screen's
//! `physical` rect says which one it is, and its own scale says what the point means in
//! record space. The page does this, from the event's css px (anchored to its own
//! window's DPI context, which still holds past its edge) plus its own screen's origin.
//! Deliberately not built on `cursor_position()`: measured on this pair, that answers in
//! a different space from window geometry — a pointer physically at 4000,667 came back as
//! 2000,333 — so a mapping built on it looks right and lands the floatie in the wrong
//! place.
//!
//! Everything here is pure: the only caller that touches the platform is
//! `screens_from`, and the arithmetic has tests that do not need two screens.

use serde::Serialize;

/// How far from the edge a floatie that comes back is placed: a window is wider than
/// this, so a point this far in is a floatie you can see and grab.
pub const MARGIN: f64 = 24.0;
/// The clear space left between two floaties that came back together.
pub const GAP: f64 = 8.0;

/// One monitor, in both spaces at once.
#[derive(Debug, Clone, Serialize)]
pub struct Screen {
    /// The device name, which is what stays put across replugs (`\\.\DISPLAY1`).
    pub key: String,
    pub name: String,
    /// Where it is, in the physical px a window is placed in.
    pub physical: Rect,
    /// Where it is, in the space records are stored in.
    pub logical: Rect,
    pub scale: f64,
    pub primary: bool,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    pub fn right(&self) -> f64 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }
}

impl Screen {
    /// A point in this screen's own css px → the virtual space.
    pub fn to_virtual(&self, x: f64, y: f64) -> (f64, f64) {
        (self.logical.x + x, self.logical.y + y)
    }


}

/// Which screen a virtual point is on, if any. `None` for the mixed-DPI band — and
/// for anything that was on a screen that is gone.
pub fn screen_of(screens: &[Screen], x: f64, y: f64) -> Option<usize> {
    screens.iter().position(|s| s.logical.contains(x, y))
}

/// The screen nearest a virtual point that is on none of them.
///
/// "Nearest" is what makes a floatie come back: a point on no screen is a point whose
/// screen was unplugged, and the honest destination is the closest screen that still
/// exists — not the primary, which may be the far one.
pub fn nearest(screens: &[Screen], x: f64, y: f64) -> Option<usize> {
    screens
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            let gap = |s: &Screen| {
                let dx = (s.logical.x - x).max(0.0).max(x - s.logical.right());
                let dy = (s.logical.y - y).max(0.0).max(y - s.logical.bottom());
                dx * dx + dy * dy
            };
            gap(a).total_cmp(&gap(b))
        })
        .map(|(index, _)| index)
}

/// The smallest rectangle holding every screen, in the virtual space: what the
/// desktop used to be, and still what `monitorArea()` reports to a widget.
pub fn union(screens: &[Screen]) -> Rect {
    if screens.is_empty() {
        return Rect::new(0.0, 0.0, 1920.0, 1080.0);
    }
    let min_x = screens
        .iter()
        .map(|s| s.logical.x)
        .fold(f64::INFINITY, f64::min);
    let min_y = screens
        .iter()
        .map(|s| s.logical.y)
        .fold(f64::INFINITY, f64::min);
    let max_x = screens
        .iter()
        .map(|s| s.logical.right())
        .fold(f64::NEG_INFINITY, f64::max);
    let max_y = screens
        .iter()
        .map(|s| s.logical.bottom())
        .fold(f64::NEG_INFINITY, f64::max);
    Rect::new(min_x, min_y, max_x - min_x, max_y - min_y)
}

/// A place inside `rect`, `margin` px from its edges, where all of a `w * h` floatie
/// fits — so one that comes back is a floatie you can see, not one clinging to the
/// edge with two thirds of itself hanging off the side.
/// Where a widget is allowed to rest: with its *whole* body on a screen, flush with the edge
/// if that is where it was put.
///
/// `screen_of` answers for the widget's top-left — which is why a widget could be dragged over
/// a screen's right edge and stay there. Its corner was on the screen and its body was not,
/// and the part past the edge is drawn by nobody: an overlay window *is* one screen, so the
/// remainder was simply invisible. Asking `clamp_into` for the whole rectangle is the whole
/// fix; the margin is 0 for a widget a person placed (flush is a legitimate choice) and the
/// caller passes `MARGIN` for one that is coming back from somewhere it should never have
/// been.
pub fn confine(
    screens: &[Screen],
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    margin: f64,
) -> Option<(f64, f64)> {
    let index = screen_of(screens, x, y).or_else(|| nearest(screens, x, y))?;
    Some(clamp_into(screens[index].logical, x, y, w, h, margin))
}

/// Is the widget's whole rectangle on one screen? The invariant `confine` restores.
pub fn within_one_screen(screens: &[Screen], x: f64, y: f64, w: f64, h: f64) -> bool {
    screens.iter().any(|s| {
        x >= s.logical.x
            && y >= s.logical.y
            && x + w <= s.logical.right()
            && y + h <= s.logical.bottom()
    })
}

pub fn clamp_into(rect: Rect, x: f64, y: f64, w: f64, h: f64, margin: f64) -> (f64, f64) {
    let left = rect.x + margin;
    let top = rect.y + margin;
    // A floatie wider or taller than the screen it is coming back to cannot fit: hold
    // it at the top-left rather than pushing it off the far edge, where the clamp
    // would be inverted and `clamp` would panic.
    let right = (rect.right() - margin - w).max(left);
    let bottom = (rect.bottom() - margin - h).max(top);
    (x.clamp(left, right).round(), y.clamp(top, bottom).round())
}

/// Two rectangles overlap (touching edges do not count).
pub fn rects_overlap(a: Rect, b: Rect) -> bool {
    a.x < b.right() && b.x < a.right() && a.y < b.bottom() && b.y < a.bottom()
}

/// A place near `start` where this floatie does not land on top of another one.
///
/// Floaties that came back from a screen that is gone all arrive from beyond the same
/// edge, so the clamp by itself puts every one of them at the same spot: three icons
/// come home and the user sees one icon and two hidden behind it. They stack down the
/// edge instead, and open a new column further in when the edge is full.
pub fn place_without_overlap(
    rect: Rect,
    w: f64,
    h: f64,
    start: (f64, f64),
    taken: &[Rect],
    margin: f64,
    gap: f64,
) -> (f64, f64) {
    let (mut x, mut y) = start;
    let bottom = rect.bottom() - margin - h;
    let left = rect.x + margin;
    for _ in 0..128 {
        if !taken
            .iter()
            .any(|other| rects_overlap(*other, Rect::new(x, y, w, h)))
        {
            return (x.round(), y.round());
        }
        y += h + gap;
        if y > bottom {
            // The column is full: one step further in, back to the row it started at.
            x -= w + gap;
            y = start.1;
            if x < left {
                // Nowhere clear: the clamped spot is still better than nowhere.
                return (start.0.round(), start.1.round());
            }
        }
    }
    (x.round(), y.round())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two monitors this project is actually developed against: a 2880x1920
    /// primary at 2.00 and a 1920x1080 screen at 1.50, side by side.
    /// The bug that was reported: a widget whose top-left is on the screen and whose body is
    /// not, because `screen_of` answers for the corner. The part past the edge is drawn by
    /// nobody — an overlay window is exactly one screen.
    #[test]
    fn a_widget_is_confined_with_its_whole_body_on_one_screen() {
        let screens = measured_pair();
        assert!(!within_one_screen(&screens, 1340.0, 300.0, 200.0, 100.0));
        let (x, y) = confine(&screens, 1340.0, 300.0, 200.0, 100.0, 0.0).unwrap();
        assert!(within_one_screen(&screens, x, y, 200.0, 100.0));
        // Flush against the edge, not away from it: this is a clamp for a widget a person
        // placed by hand, so nothing gains a margin it did not ask for.
        assert_eq!((x, y), (1240.0, 300.0));
    }

    #[test]
    fn a_widget_past_the_right_of_everything_comes_to_the_nearest_screen() {
        let screens = measured_pair();
        // Past the right edge of the right screen, so on no screen at all: 100px from
        // DISPLAY2 and 1860px from DISPLAY1.
        let (x, y) = confine(&screens, 3300.0, 300.0, 200.0, 100.0, 0.0).unwrap();
        assert!(within_one_screen(&screens, x, y, 200.0, 100.0));
        assert_eq!((x, y), (3000.0, 300.0));
    }

    #[test]
    fn a_widget_too_big_for_the_screen_is_held_at_its_corner() {
        let screens = measured_pair();
        // `clamp_into`'s own rule, reached through `confine`: a body larger than the screen
        // would invert the clamp and panic, so it is held at the top-left instead.
        assert_eq!(
            confine(&screens, 20.0, 20.0, 2000.0, 1200.0, 0.0).unwrap(),
            (0.0, 0.0)
        );
    }

    fn measured_pair() -> Vec<Screen> {
        vec![
            Screen {
                key: "\\\\.\\DISPLAY1".to_string(),
                name: "\\\\.\\DISPLAY1".to_string(),
                physical: Rect::new(0.0, 0.0, 2880.0, 1920.0),
                logical: Rect::new(0.0, 0.0, 1440.0, 960.0),
                scale: 2.0,
                primary: true,
            },
            Screen {
                key: "\\\\.\\DISPLAY2".to_string(),
                name: "\\\\.\\DISPLAY2".to_string(),
                physical: Rect::new(2880.0, 0.0, 1920.0, 1080.0),
                logical: Rect::new(1920.0, 0.0, 1280.0, 720.0),
                scale: 1.5,
                primary: false,
            },
        ]
    }

    fn alone() -> Vec<Screen> {
        let mut screens = measured_pair();
        screens.truncate(1);
        screens
    }

    #[test]
    fn a_point_belongs_to_the_screen_it_falls_on() {
        let screens = measured_pair();
        assert_eq!(screen_of(&screens, 100.0, 100.0), Some(0));
        assert_eq!(screen_of(&screens, 2000.0, 100.0), Some(1));
        // The mixed-DPI band belongs to no screen: it is the 480px between where the
        // primary ends in virtual px and where the second screen starts.
        assert_eq!(screen_of(&screens, 1600.0, 100.0), None);
        // ... and neither does a point below both.
        assert_eq!(screen_of(&screens, 100.0, 2000.0), None);
    }

    #[test]
    fn a_floatie_off_every_screen_comes_back_to_the_nearest_one() {
        let screens = measured_pair();
        // On a screen: untouched.
        assert_eq!(nearest(&screens, 100.0, 100.0), Some(0));
        assert_eq!(nearest(&screens, 2000.0, 100.0), Some(1));

        // The second screen is unplugged. A floatie that was on it comes back to the
        // nearest edge of the screen that still exists — not to the primary's corner,
        // which is where "clamp to the desktop" would have put it.
        let alone = alone();
        let there = union(&alone);
        assert_eq!(nearest(&alone, 2000.0, 120.0), Some(0));
        assert_eq!(clamp_into(there, 2000.0, 120.0, 92.0, 112.0, 24.0), (1324.0, 120.0));
        assert!(
            1324.0 + 92.0 <= there.right(),
            "the whole icon is on the screen, not just its corner"
        );

        // One from above a screen comes down into it, keeping its column ...
        assert_eq!(clamp_into(there, 300.0, -400.0, 92.0, 112.0, 24.0), (300.0, 24.0));
        // ... and a panel as tall as most of the screen fits by its own height.
        assert_eq!(clamp_into(there, 2000.0, 5000.0, 300.0, 330.0, 24.0), (1116.0, 606.0));
        // Something bigger than the screen cannot be fitted: it is held at the
        // top-left instead of being pushed off the far edge.
        assert_eq!(clamp_into(there, 2000.0, 5000.0, 5000.0, 5000.0, 24.0), (24.0, 24.0));

        // Three screens, including one to the left: nearest is nearest, not first.
        let mut three = measured_pair();
        three.insert(
            0,
            Screen {
                key: "left".to_string(),
                name: "left".to_string(),
                physical: Rect::new(-1920.0, 0.0, 1920.0, 1536.0),
                logical: Rect::new(-1280.0, 0.0, 1280.0, 1024.0),
                scale: 1.5,
                primary: false,
            },
        );
        assert_eq!(nearest(&three, -3000.0, 100.0), Some(0));
        assert_eq!(nearest(&three, 2600.0, 100.0), Some(2));
        assert_eq!(
            nearest(&three, 1000.0, 2000.0),
            Some(1),
            "below the middle screen, nearer to it than to either neighbour"
        );
        assert_eq!(
            clamp_into(three[0].logical, -3000.0, 100.0, 92.0, 112.0, 24.0),
            (-1256.0, 100.0)
        );
    }

    #[test]
    fn the_desktop_is_the_smallest_box_holding_every_screen() {
        assert_eq!(union(&alone()), Rect::new(0.0, 0.0, 1440.0, 960.0));
        // The measured pair, in the virtual space Windows hands over: the second
        // screen's origin is physical px over *its* scale, which is why the union is
        // wider than the two screens added up (and why the band exists).
        assert_eq!(union(&measured_pair()), Rect::new(0.0, 0.0, 3200.0, 960.0));
        assert_eq!(union(&[]), Rect::new(0.0, 0.0, 1920.0, 1080.0));
    }

    #[test]
    fn a_screen_maps_points_both_ways_through_its_own_scale() {
        let screens = measured_pair();
        let second = &screens[1];
        // A css point in the second screen's page → the virtual space …
        assert_eq!(second.to_virtual(0.0, 0.0), (1920.0, 0.0));
        assert_eq!(second.to_virtual(1280.0, 720.0), (3200.0, 720.0));
        // … and the mapping is what the page does in js: css px are screen-relative,
        // so the drag that leaves this screen keeps counting from its own origin.
        // A virtual point past the screen's own right edge is *not* contained.
        assert!(!second.logical.contains(3300.0, 100.0));
        assert!(second.logical.contains(3199.0, 100.0));
    }

    #[test]
    fn floaties_that_come_back_together_do_not_land_on_top_of_each_other() {
        let rect = Rect::new(0.0, 0.0, 1440.0, 960.0);
        let (w, h) = (92.0, 112.0);
        // Three icons from the screen that went away: the clamp puts all three here,
        // which is one visible icon and two hidden behind it.
        let start = clamp_into(rect, 2000.0, 100.0, w, h, 24.0);
        assert_eq!(start, (1324.0, 100.0));

        let mut taken: Vec<Rect> = Vec::new();
        let first = place_without_overlap(rect, w, h, start, &taken, 24.0, 8.0);
        taken.push(Rect::new(first.0, first.1, w, h));
        let second = place_without_overlap(rect, w, h, start, &taken, 24.0, 8.0);
        taken.push(Rect::new(second.0, second.1, w, h));
        let third = place_without_overlap(rect, w, h, start, &taken, 24.0, 8.0);

        assert_eq!(first, start, "the first one keeps the spot it was clamped to");
        assert_eq!(second.1, 100.0 + 112.0 + 8.0, "the second goes below it");
        assert_eq!(third.1, 100.0 + 2.0 * (112.0 + 8.0), "the third below that");
        for (x, y) in [first, second, third] {
            assert!(x >= 0.0 && x + w <= rect.right(), "fully on the screen: {x}");
            assert!(y >= 0.0 && y + h <= rect.bottom(), "fully on the screen: {y}");
        }
        let placed = [
            Rect::new(first.0, first.1, w, h),
            Rect::new(second.0, second.1, w, h),
            Rect::new(third.0, third.1, w, h),
        ];
        for a in 0..placed.len() {
            for b in (a + 1)..placed.len() {
                assert!(!rects_overlap(placed[a], placed[b]), "{a} overlaps {b}");
            }
        }

        // A column that runs out of room continues further in, and stays inside.
        let near_bottom = place_without_overlap(
            rect,
            w,
            h,
            (1324.0, 800.0),
            &[
                Rect::new(1324.0, 800.0, w, h),
                Rect::new(1324.0, 920.0, w, h),
            ],
            24.0,
            8.0,
        );
        assert!(near_bottom.0 + w <= rect.right() && near_bottom.1 + h <= rect.bottom());
        assert!(!rects_overlap(
            Rect::new(near_bottom.0, near_bottom.1, w, h),
            Rect::new(1324.0, 800.0, w, h)
        ));
    }
}
