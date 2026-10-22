//! Medición de la rasterización (`--benchmark-triangles`): solo la generación de fragmentos, sin
//! textura, ventana ni esperas entre cuadros. Se corre con `cargo run --release`.

use crate::raster::{rasterize, Triangle, TriangleMode};
use std::hint::black_box;
use std::time::{Duration, Instant};

/// Mediciones por algoritmo y carga; se reporta la mediana.
const REPS: usize = 11;
/// Cada medición dura aproximadamente esto (se ajusta el número de ejecuciones).
const TARGET: Duration = Duration::from_millis(150);

/// Triángulos pseudoaleatorios (siempre los mismos para una semilla): cada uno cabe en un cuadrado
/// de `size` píxeles ubicado al azar dentro de una imagen de `w` x `h`.
pub fn synthetic(count: usize, size: i32, w: i32, h: i32, seed: u64) -> Vec<Triangle> {
    let mut s = seed.wrapping_mul(0x9E3779B97F4A7C15) | 1;
    let mut next = |max: i32| -> i32 {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((s >> 33) % max.max(1) as u64) as i32
    };
    (0..count)
        .map(|i| {
            let (ox, oy) = (next(w - size), next(h - size));
            let mut p = || (ox + next(size), oy + next(size));
            let shade = (60 + i * 7 % 190) as u8;
            Triangle { v: [p(), p(), p()], color: [shade, 200, 255, 255] }
        })
        .collect()
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

/// Mide cada modo (o solo `only`, si se pide uno) sobre cada carga y escribe una tabla en Markdown.
pub fn run(workloads: &[(String, Vec<Triangle>)], only: Option<TriangleMode>) {
    println!("Mediana de {REPS} mediciones, cada una con varias ejecuciones; solo `rasterize` (fragmentos).");
    println!();
    println!("| Carga | Triángulos | Modo | Fragmentos | Tiempo por ejecución | ns por fragmento |");
    println!("|---|---:|---|---:|---:|---:|");
    let mut filled_counts = Vec::new();
    for (name, triangles) in workloads {
        let mut by_mode: Vec<(TriangleMode, usize, Duration)> = Vec::new();
        for mode in TriangleMode::ALL.into_iter().filter(|m| only.is_none_or(|o| o == *m)) {
            // Calentamiento y conteo de fragmentos (el resultado se usa, no se descarta).
            let fragments = rasterize(black_box(triangles), mode);
            let count = fragments.len();
            black_box(fragments);

            // Elegir cuántas ejecuciones caben en ~TARGET.
            let t0 = Instant::now();
            for _ in 0..3 {
                black_box(rasterize(black_box(triangles), mode));
            }
            let one = (t0.elapsed() / 3).max(Duration::from_nanos(1));
            let runs = ((TARGET.as_nanos() / one.as_nanos()) as usize).clamp(3, 5000);

            let samples: Vec<Duration> = (0..REPS)
                .map(|_| {
                    let start = Instant::now();
                    for _ in 0..runs {
                        let fragments = rasterize(black_box(triangles), mode);
                        black_box(fragments);
                    }
                    start.elapsed() / runs as u32
                })
                .collect();
            by_mode.push((mode, count, median(samples)));
        }
        for (mode, count, t) in &by_mode {
            let ns = t.as_nanos() as f64 / (*count).max(1) as f64;
            println!(
                "| {name} | {} | {} | {count} | {t:.2?} | {ns:.2} |",
                triangles.len(),
                mode.name()
            );
        }
        if let Some((_, count, _)) = by_mode.iter().find(|(m, _, _)| *m != TriangleMode::Wireframe) {
            filled_counts.push((name, triangles, *count));
        }
    }

    // Cuánto trabajo evalúa cada algoritmo para producir los mismos fragmentos.
    println!();
    println!("| Carga | Fragmentos (relleno) | Píxeles evaluados por baricéntrico | Aprovechamiento | Filas recorridas por scanline |");
    println!("|---|---:|---:|---:|---:|");
    for (name, triangles, count) in filled_counts {
        let (mut rect_pixels, mut rows) = (0i64, 0i64);
        for t in triangles {
            let xs = t.v.map(|p| p.0);
            let ys = t.v.map(|p| p.1);
            let (x0, x1) = (xs.into_iter().min().unwrap(), xs.into_iter().max().unwrap());
            let (y0, y1) = (ys.into_iter().min().unwrap(), ys.into_iter().max().unwrap());
            rect_pixels += (x1 - x0) as i64 * (y1 - y0) as i64;
            rows += (y1 - y0) as i64;
        }
        let ratio = 100.0 * count as f64 / rect_pixels.max(1) as f64;
        println!("| {name} | {count} | {rect_pixels} | {ratio:.0} % | {rows} |");
    }
}