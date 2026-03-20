use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Color,
    symbols::Marker,
    widgets::{Widget, canvas::Canvas},
};

#[derive(Default)]
pub struct Bolt;

impl Widget for Bolt {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Bolt outline
        const BOLT: [(f64, f64); 7] = [
            (0.60, 0.05),
            (0.42, 0.40),
            (0.64, 0.40),
            (0.26, 0.95),
            (0.50, 0.55),
            (0.32, 0.55),
            (0.60, 0.05),
        ];
        let area = Rect {
            x: area.x + area.width / 15,
            y: area.y + area.height / 4,
            width: area.width,
            height: area.height / 2,
        };

        // fill the bolt with dense points using braille marker (2x4 subcells per cell)
        Canvas::default()
            .x_bounds([0.0, 1.0])
            .y_bounds([0.0, 1.0])
            .marker(Marker::Braille)
            .paint(|ctx| {
                let mut pts: Vec<(f64, f64)> = Vec::new();

                // sampling density (increase if you want smoother)
                const SX: usize = 160; // sub-samples across X
                const SY: usize = 320; // sub-samples across Y

                for iy in 0..SY {
                    let y = (iy as f64 + 0.5) / SY as f64;
                    // find polygon-edge intersections with this scanline
                    let mut xs: Vec<f64> = Vec::new();
                    for i in 0..BOLT.len() - 1 {
                        let (x1, y1) = BOLT[i];
                        let (x2, y2) = BOLT[i + 1];
                        if (y1 > y) != (y2 > y) && (y2 - y1).abs() > f64::EPSILON {
                            let t = (y - y1) / (y2 - y1);
                            xs.push(x1 + t * (x2 - x1));
                        }
                    }
                    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());

                    // fill between pairs of intersections with sub-sampled points
                    for pair in xs.chunks(2) {
                        if pair.len() < 2 {
                            continue;
                        }
                        let (x0, x1) = (pair[0], pair[1]);
                        let steps = ((x1 - x0) * SX as f64).max(1.0).ceil() as usize;
                        for s in 0..steps {
                            let x = x0 + (s as f64 + 0.5) / SX as f64;
                            pts.push((x, y));
                        }
                    }
                }

                ctx.draw(&ratatui::widgets::canvas::Points {
                    coords: pts.as_slice(),
                    color: Color::Yellow,
                });
            })
            .render(area, buf);
    }
}
