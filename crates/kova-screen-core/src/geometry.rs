use serde::{Deserialize, Serialize};

/// A point in physical (device) pixels on the virtual desktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

impl Size {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    pub const fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Total pixel count as `u64` so callers can bounds-check without overflow.
    pub const fn pixel_count(&self) -> u64 {
        self.width as u64 * self.height as u64
    }
}

/// An axis-aligned rectangle in physical pixels.
///
/// `x`/`y` may be negative: the Windows virtual desktop places secondary
/// monitors at negative coordinates when they sit left of or above the primary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Builds a rect from two corners in any order, as produced by a drag.
    pub fn from_corners(a: Point, b: Point) -> Self {
        let x = a.x.min(b.x);
        let y = a.y.min(b.y);
        // Differences of two i32 can overflow i32, so widen before subtracting.
        let width = (a.x as i64 - b.x as i64).unsigned_abs() as u32;
        let height = (a.y as i64 - b.y as i64).unsigned_abs() as u32;
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub const fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }

    pub const fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    pub const fn left(&self) -> i32 {
        self.x
    }

    pub const fn top(&self) -> i32 {
        self.y
    }

    pub fn right(&self) -> i32 {
        self.x.saturating_add(self.width as i32)
    }

    pub fn bottom(&self) -> i32 {
        self.y.saturating_add(self.height as i32)
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.left() && p.x < self.right() && p.y >= self.top() && p.y < self.bottom()
    }

    /// Intersection of two rects, or `None` when they do not overlap.
    pub fn intersect(&self, other: &Rect) -> Option<Rect> {
        let x = self.left().max(other.left());
        let y = self.top().max(other.top());
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        if right <= x || bottom <= y {
            return None;
        }
        Some(Rect::new(x, y, (right - x) as u32, (bottom - y) as u32))
    }

    /// Translates the rect so it is expressed relative to `origin`.
    pub fn to_local(&self, origin: Point) -> Rect {
        Rect::new(
            self.x - origin.x,
            self.y - origin.y,
            self.width,
            self.height,
        )
    }

    /// Rounds width and height down to the nearest multiple of `n`.
    ///
    /// H.264 requires even dimensions; some hardware encoders want multiples of
    /// 4 or 16. Never grows the rect, so it can't run past a monitor edge.
    pub fn align_down(&self, n: u32) -> Rect {
        debug_assert!(n > 0);
        Rect::new(
            self.x,
            self.y,
            self.width - self.width % n,
            self.height - self.height % n,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_corners_normalises_any_drag_direction() {
        let a = Point::new(300, 200);
        let b = Point::new(100, 50);
        let expected = Rect::new(100, 50, 200, 150);
        assert_eq!(Rect::from_corners(a, b), expected);
        assert_eq!(Rect::from_corners(b, a), expected);
    }

    #[test]
    fn from_corners_handles_negative_virtual_desktop_coords() {
        // Monitor left of the primary display sits at negative x.
        let r = Rect::from_corners(Point::new(-1920, -100), Point::new(-1700, 300));
        assert_eq!(r, Rect::new(-1920, -100, 220, 400));
    }

    #[test]
    fn from_corners_does_not_overflow_on_extreme_coords() {
        let r = Rect::from_corners(Point::new(i32::MIN, 0), Point::new(i32::MAX, 0));
        assert_eq!(r.width, u32::MAX);
        assert_eq!(r.height, 0);
    }

    #[test]
    fn intersect_returns_none_when_disjoint() {
        let a = Rect::new(0, 0, 100, 100);
        let b = Rect::new(200, 200, 50, 50);
        assert_eq!(a.intersect(&b), None);
    }

    #[test]
    fn intersect_clips_to_overlap() {
        let a = Rect::new(0, 0, 100, 100);
        let b = Rect::new(50, 50, 100, 100);
        assert_eq!(a.intersect(&b), Some(Rect::new(50, 50, 50, 50)));
    }

    #[test]
    fn touching_edges_do_not_intersect() {
        let a = Rect::new(0, 0, 100, 100);
        let b = Rect::new(100, 0, 100, 100);
        assert_eq!(a.intersect(&b), None);
    }

    #[test]
    fn align_down_never_grows_the_rect() {
        let r = Rect::new(10, 10, 1919, 1081).align_down(2);
        assert_eq!(r, Rect::new(10, 10, 1918, 1080));
        let r16 = Rect::new(0, 0, 100, 100).align_down(16);
        assert!(r16.width <= 100 && r16.height <= 100);
        assert_eq!(r16.width % 16, 0);
    }

    #[test]
    fn to_local_rebases_onto_monitor_origin() {
        let monitor = Point::new(-1920, 0);
        let selection = Rect::new(-1800, 100, 400, 300);
        assert_eq!(selection.to_local(monitor), Rect::new(120, 100, 400, 300));
    }
}
