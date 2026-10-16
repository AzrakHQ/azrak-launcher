//! Shared, testable Library geometry. UI dimensions and virtualized scrolling use one source.
pub const TOP: f32 = 324.0;

pub struct Grid {
    pub cols: usize,
    pub card: f32,
    pub row: f32,
    pub gap: f32,
    pub typography: f32,
}

impl Grid {
    pub fn new(width: f32, scale: f32, compact: bool) -> Self {
        let scale = scale.clamp(0.3, 4.0);
        let typography = scale.max(0.75);
        let inner = (width - 192.0).max(400.0);
        let gap = if compact { 24.0 } else { 28.0 };
        // Respect real window pixels, not just a constantly downscaled 1920px canvas.
        let minimum = if compact { 190.0f32.max(126.0 / scale) } else { 214.0f32.max(150.0 / scale) };
        let cols = (((inner + gap) / (minimum + gap)).floor() as usize).clamp(2, 16);
        let card = (inner - (cols as f32 - 1.0) * gap) / cols as f32;
        let row = card * 1.5 + 102.0 * typography / scale + 42.0;
        Self { cols, card, row, gap, typography }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn comfortable_is_roomier_than_compact() {
        let a = Grid::new(1920.0, 1.0, false);
        let b = Grid::new(1920.0, 1.0, true);
        assert_eq!((a.cols, b.cols), (7, 8));
        assert!(a.card > b.card);
    }
    #[test]
    fn narrow_windows_reduce_columns_without_shrinking_text() {
        let full = Grid::new(1920.0, 1.0, false);
        let narrow = Grid::new(1920.0, 0.5, false);
        assert!(narrow.cols < full.cols);
        assert!(narrow.card * 0.5 >= 150.0);
        assert!(20.0 * narrow.typography >= 15.0);
    }
    #[test]
    fn row_reserves_two_title_lines_and_all_metadata() {
        for scale in [0.3, 0.5, 0.7, 1.0, 2.0] {
            for compact in [false, true] {
                let g = Grid::new(1920.0, scale, compact);
                assert!(g.row * scale >= g.card * 1.5 * scale + 102.0 * g.typography + 41.0 * scale);
                assert!(g.card.is_finite() && g.card > 0.0);
            }
        }
    }
    #[test]
    fn desktop_and_native_logo_stay_identical() {
        assert_eq!(include_bytes!("../assets/azrak.svg").as_slice(), include_bytes!("../assets/icons/app-icon.svg").as_slice());
    }
}