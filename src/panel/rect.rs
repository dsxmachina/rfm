//! Rectangle type for defining UI regions.
//!
//! This module provides a simple `Rect` type that encapsulates the x/y ranges
//! currently used throughout the panel rendering code. It provides a cleaner
//! API and prepares for the compositor/widget refactoring.

use std::ops::Range;

/// A rectangle defined by position (x, y) and size (width, height).
///
/// This is used to define regions on the terminal screen for rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl Rect {
    /// Create a new rectangle.
    pub fn new(x: u16, y: u16, width: u16, height: u16) -> Self {
        Self { x, y, width, height }
    }

    /// Create a rectangle from x and y ranges.
    ///
    /// This provides compatibility with the existing Range<u16> API.
    pub fn from_ranges(x_range: Range<u16>, y_range: Range<u16>) -> Self {
        Self {
            x: x_range.start,
            y: y_range.start,
            width: x_range.end.saturating_sub(x_range.start),
            height: y_range.end.saturating_sub(y_range.start),
        }
    }

    /// Create a zero-sized rectangle at the origin.
    pub fn zero() -> Self {
        Self { x: 0, y: 0, width: 0, height: 0 }
    }

    /// Get the x range (for compatibility with existing code).
    #[inline]
    pub fn x_range(&self) -> Range<u16> {
        self.x..self.right()
    }

    /// Get the y range (for compatibility with existing code).
    #[inline]
    pub fn y_range(&self) -> Range<u16> {
        self.y..self.bottom()
    }

    /// Get the right edge (exclusive).
    #[inline]
    pub fn right(&self) -> u16 {
        self.x.saturating_add(self.width)
    }

    /// Get the bottom edge (exclusive).
    #[inline]
    pub fn bottom(&self) -> u16 {
        self.y.saturating_add(self.height)
    }

    /// Get the left edge.
    #[inline]
    pub fn left(&self) -> u16 {
        self.x
    }

    /// Get the top edge.
    #[inline]
    pub fn top(&self) -> u16 {
        self.y
    }

    /// Get the area of this rectangle.
    #[inline]
    pub fn area(&self) -> u32 {
        self.width as u32 * self.height as u32
    }

    /// Check if this rectangle is empty (zero area).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Check if this rectangle contains a point.
    pub fn contains(&self, x: u16, y: u16) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    /// Check if this rectangle intersects with another.
    pub fn intersects(&self, other: &Rect) -> bool {
        self.x < other.right()
            && self.right() > other.x
            && self.y < other.bottom()
            && self.bottom() > other.y
    }

    /// Get the intersection of this rectangle with another.
    /// Returns None if they don't intersect.
    pub fn intersection(&self, other: &Rect) -> Option<Rect> {
        if !self.intersects(other) {
            return None;
        }

        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());

        Some(Rect {
            x,
            y,
            width: right.saturating_sub(x),
            height: bottom.saturating_sub(y),
        })
    }

    /// Create a new rectangle by shrinking this one by the given margin.
    pub fn inner(&self, margin: u16) -> Rect {
        let double_margin = margin.saturating_mul(2);
        Rect {
            x: self.x.saturating_add(margin),
            y: self.y.saturating_add(margin),
            width: self.width.saturating_sub(double_margin),
            height: self.height.saturating_sub(double_margin),
        }
    }

    /// Split this rectangle horizontally at the given offset from the top.
    /// Returns (top_rect, bottom_rect).
    pub fn split_horizontal(&self, at: u16) -> (Rect, Rect) {
        let at = at.min(self.height);
        (
            Rect {
                x: self.x,
                y: self.y,
                width: self.width,
                height: at,
            },
            Rect {
                x: self.x,
                y: self.y.saturating_add(at),
                width: self.width,
                height: self.height.saturating_sub(at),
            },
        )
    }

    /// Split this rectangle vertically at the given offset from the left.
    /// Returns (left_rect, right_rect).
    pub fn split_vertical(&self, at: u16) -> (Rect, Rect) {
        let at = at.min(self.width);
        (
            Rect {
                x: self.x,
                y: self.y,
                width: at,
                height: self.height,
            },
            Rect {
                x: self.x.saturating_add(at),
                y: self.y,
                width: self.width.saturating_sub(at),
                height: self.height,
            },
        )
    }
}

impl Default for Rect {
    fn default() -> Self {
        Self::zero()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_ranges() {
        let rect = Rect::from_ranges(10..30, 5..25);
        assert_eq!(rect.x, 10);
        assert_eq!(rect.y, 5);
        assert_eq!(rect.width, 20);
        assert_eq!(rect.height, 20);
    }

    #[test]
    fn test_x_y_ranges() {
        let rect = Rect::new(10, 5, 20, 15);
        assert_eq!(rect.x_range(), 10..30);
        assert_eq!(rect.y_range(), 5..20);
    }

    #[test]
    fn test_edges() {
        let rect = Rect::new(10, 5, 20, 15);
        assert_eq!(rect.left(), 10);
        assert_eq!(rect.top(), 5);
        assert_eq!(rect.right(), 30);
        assert_eq!(rect.bottom(), 20);
    }

    #[test]
    fn test_contains() {
        let rect = Rect::new(10, 10, 20, 20);
        assert!(rect.contains(10, 10));
        assert!(rect.contains(15, 15));
        assert!(rect.contains(29, 29));
        assert!(!rect.contains(30, 30));
        assert!(!rect.contains(9, 10));
    }

    #[test]
    fn test_intersects() {
        let a = Rect::new(0, 0, 10, 10);
        let b = Rect::new(5, 5, 10, 10);
        let c = Rect::new(20, 20, 10, 10);

        assert!(a.intersects(&b));
        assert!(b.intersects(&a));
        assert!(!a.intersects(&c));
    }

    #[test]
    fn test_intersection() {
        let a = Rect::new(0, 0, 10, 10);
        let b = Rect::new(5, 5, 10, 10);

        let intersection = a.intersection(&b).unwrap();
        assert_eq!(intersection, Rect::new(5, 5, 5, 5));
    }

    #[test]
    fn test_inner() {
        let rect = Rect::new(10, 10, 20, 20);
        let inner = rect.inner(2);
        assert_eq!(inner, Rect::new(12, 12, 16, 16));
    }

    #[test]
    fn test_split_horizontal() {
        let rect = Rect::new(0, 0, 100, 100);
        let (top, bottom) = rect.split_horizontal(30);
        assert_eq!(top, Rect::new(0, 0, 100, 30));
        assert_eq!(bottom, Rect::new(0, 30, 100, 70));
    }

    #[test]
    fn test_split_vertical() {
        let rect = Rect::new(0, 0, 100, 100);
        let (left, right) = rect.split_vertical(40);
        assert_eq!(left, Rect::new(0, 0, 40, 100));
        assert_eq!(right, Rect::new(40, 0, 60, 100));
    }
}
