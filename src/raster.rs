//! Rasterización de triángulos: convierte tres vértices en pantalla en una lista de fragmentos
//! (posición + color) sin tocar el framebuffer. Así se puede cambiar de algoritmo sin modificar
//! el ensamblaje de primitivas ni el procesamiento de fragmentos.
//!
//! Los vértices están en píxeles enteros y el centro del píxel (x, y) es (x + 0.5, y + 0.5).
//! Los dos rellenos usan aritmética entera exacta y la misma regla de bordes (ver `README.md`):
//!
//! * Una fila `y` pertenece al triángulo si su centro cae en [y_mín, y_máx): el borde superior
//!   cuenta y el inferior no.
//! * Dentro de una fila, un píxel cuenta si su centro está en [x_izq, x_der): el borde izquierdo
//!   cuenta y el derecho no.
//!
//! Es la regla "top-left" de las GPU: un borde compartido por dos triángulos lo dibuja exactamente
//! uno de los dos, sin huecos ni fragmentos duplicados.

/// Posición en pantalla (x, y) en píxeles; y crece hacia abajo.
pub type Point = (i32, i32);

/// Un píxel generado por la rasterización, todavía sin escribir en el framebuffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fragment {
    pub x: i32,
    pub y: i32,
    pub color: [u8; 4],
}

/// Triángulo ya proyectado a pantalla, con un color uniforme.
#[derive(Clone, Copy, Debug)]
pub struct Triangle {
    pub v: [Point; 3],
    pub color: [u8; 4],
}

/// Cómo se generan los fragmentos de un triángulo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriangleMode {
    Wireframe,
    FilledBarycentric,
    FilledScanline,
}

impl TriangleMode {
    pub const ALL: [TriangleMode; 3] = [
        TriangleMode::Wireframe,
        TriangleMode::FilledBarycentric,
        TriangleMode::FilledScanline,
    ];

    /// Nombre corto, usado en la línea de comandos, en los nombres de archivo y en las tablas.
    pub fn name(self) -> &'static str {
        match self {
            TriangleMode::Wireframe => "wireframe",
            TriangleMode::FilledBarycentric => "barycentric",
            TriangleMode::FilledScanline => "scanline",
        }
    }

    pub fn from_name(s: &str) -> Option<TriangleMode> {
        TriangleMode::ALL.into_iter().find(|m| m.name() == s)
    }
}

/// Algoritmo de Bresenham: llama a `plot` por cada píxel de la línea (incluye ambos extremos).
pub fn bresenham(x0: i32, y0: i32, x1: i32, y1: i32, mut plot: impl FnMut(i32, i32)) {
    let (mut x, mut y) = (x0, y0);
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        plot(x, y);
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

/// Fragmentos de un triángulo para el modo `mode`.
pub fn triangle_fragments(a: Point, b: Point, c: Point, color: [u8; 4], mode: TriangleMode) -> Vec<Fragment> {
    match mode {
        TriangleMode::Wireframe => wireframe(a, b, c, color),
        TriangleMode::FilledBarycentric => fill_barycentric(a, b, c, color),
        TriangleMode::FilledScanline => fill_scanline(a, b, c, color),
    }
}

/// Rasteriza una lista de triángulos en el mismo orden en que vienen (importa para el algoritmo
/// del pintor: el último fragmento escrito en un píxel es el que se ve).
pub fn rasterize(triangles: &[Triangle], mode: TriangleMode) -> Vec<Fragment> {
    let mut out = Vec::new();
    for t in triangles {
        out.extend(triangle_fragments(t.v[0], t.v[1], t.v[2], t.color, mode));
    }
    out
}

/// Contorno: tres líneas de Bresenham entre los vértices.
#[inline(never)] // función propia en el flamegraph
pub fn wireframe(a: Point, b: Point, c: Point, color: [u8; 4]) -> Vec<Fragment> {
    let mut out = Vec::new();
    for (p, q) in [(a, b), (b, c), (c, a)] {
        bresenham(p.0, p.1, q.0, q.1, |x, y| out.push(Fragment { x, y, color }));
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Algoritmo 1: coordenadas baricéntricas
// ---------------------------------------------------------------------------------------------

/// Doble de la función de borde E(p) = (b - a) x (p - a), evaluada en p = (px2 / 2, py2 / 2).
/// Se trabaja con coordenadas duplicadas para que el centro del píxel (x + 0.5) sea entero.
fn edge2(a: Point, b: Point, px2: i64, py2: i64) -> i64 {
    (b.0 - a.0) as i64 * (py2 - 2 * a.1 as i64) - (b.1 - a.1) as i64 * (px2 - 2 * a.0 as i64)
}

/// Regla top-left para un borde p -> q de un triángulo con área positiva (sentido horario en
/// pantalla, y hacia abajo): es borde superior (horizontal, hacia la derecha) o izquierdo (sube).
fn is_top_left(p: Point, q: Point) -> bool {
    let (dx, dy) = (q.0 - p.0, q.1 - p.1);
    (dy == 0 && dx > 0) || dy < 0
}

/// Relleno con coordenadas baricéntricas: recorre el rectángulo que contiene al triángulo y, para
/// cada centro de píxel, calcula los tres pesos (λ_i = w_i / área). El píxel está dentro si los
/// tres pesos son positivos; si alguno es cero (el centro cae justo sobre un borde), solo cuenta
/// cuando ese borde es superior o izquierdo.
///
/// El orden de los vértices no importa: si el área sale negativa se intercambian dos vértices. Si
/// el área es cero (vértices colineales o repetidos) el triángulo es degenerado y no genera nada.
#[inline(never)] // función propia en el flamegraph
pub fn fill_barycentric(a: Point, b: Point, c: Point, color: [u8; 4]) -> Vec<Fragment> {
    let area2 = edge2(a, b, 2 * c.0 as i64, 2 * c.1 as i64); // 2 * área con signo, centro = vértice c
    if area2 == 0 {
        return Vec::new();
    }
    // Con área negativa, intercambiar b y c invierte el sentido y deja los pesos positivos.
    let (b, c) = if area2 < 0 { (c, b) } else { (b, c) };

    let min_x = a.0.min(b.0).min(c.0);
    let max_x = a.0.max(b.0).max(c.0);
    let min_y = a.1.min(b.1).min(c.1);
    let max_y = a.1.max(b.1).max(c.1);

    let (tl_bc, tl_ca, tl_ab) = (is_top_left(b, c), is_top_left(c, a), is_top_left(a, b));
    let inside = |w: i64, top_left: bool| w > 0 || (w == 0 && top_left);

    let mut out = Vec::new();
    // Los centros posibles van de min + 0.5 a max - 0.5, así que el rango es [min, max).
    for y in min_y..max_y {
        let py2 = 2 * y as i64 + 1;
        for x in min_x..max_x {
            let px2 = 2 * x as i64 + 1;
            let w0 = edge2(b, c, px2, py2); // peso del vértice a
            let w1 = edge2(c, a, px2, py2); // peso del vértice b
            let w2 = edge2(a, b, px2, py2); // peso del vértice c
            if inside(w0, tl_bc) && inside(w1, tl_ca) && inside(w2, tl_ab) {
                out.push(Fragment { x, y, color });
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Algoritmo 2: líneas horizontales (scanline)
// ---------------------------------------------------------------------------------------------

/// Primer píxel de la fila `y` cuyo centro está a la derecha o sobre la arista p -> q, es decir
/// ceil(x_arista - 0.5), calculado con enteros. Requiere p.1 < q.1 y p.1 <= y < q.1.
fn first_x(p: Point, q: Point, y: i32) -> i32 {
    let dy = (q.1 - p.1) as i64;
    // x_arista - 0.5 = num / den, con den > 0
    let num = 2 * p.0 as i64 * dy + (q.0 - p.0) as i64 * (2 * y as i64 + 1 - 2 * p.1 as i64) - dy;
    let den = 2 * dy;
    (num + den - 1).div_euclid(den) as i32 // techo
}

/// Relleno por líneas horizontales: ordena los vértices por y; para cada fila cuyo centro cae en
/// [y_mín, y_máx) cruza la arista larga (v0-v2) con la corta (v0-v1 arriba de v1, v1-v2 debajo) y
/// genera los píxeles entre los dos cruces, con el izquierdo incluido y el derecho excluido.
///
/// Las aristas horizontales nunca se cruzan (solo se evalúan aristas con y_inicio <= fila < y_fin).
/// Un vértice tiene y entera, el límite entre dos filas, y cada fila se evalúa en su centro
/// (y + 0.5): cada fila usa la arista corta de arriba si y < v1.y y la de abajo si no, así que no hay
/// huecos ni filas repetidas. Los vértices colineales dan dos cruces idénticos y un tramo vacío.
#[inline(never)] // función propia en el flamegraph
pub fn fill_scanline(a: Point, b: Point, c: Point, color: [u8; 4]) -> Vec<Fragment> {
    let mut v = [a, b, c];
    v.sort_by_key(|p| (p.1, p.0));
    let [v0, v1, v2] = v;

    let mut out = Vec::new();
    for y in v0.1..v2.1 {
        let (s0, s1) = if y < v1.1 { (v0, v1) } else { (v1, v2) };
        let long = first_x(v0, v2, y);
        let short = first_x(s0, s1, y);
        let (x_start, x_end) = if long <= short { (long, short) } else { (short, long) };
        for x in x_start..x_end {
            out.push(Fragment { x, y, color });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    const C: [u8; 4] = [255, 255, 255, 255];

    fn positions(frags: &[Fragment]) -> BTreeSet<(i32, i32)> {
        frags.iter().map(|f| (f.x, f.y)).collect()
    }

    /// Ambos rellenos deben generar exactamente las mismas posiciones, sin duplicados, y el mismo
    /// resultado con cualquier orden de los vértices. Devuelve las posiciones.
    fn check_same(a: Point, b: Point, c: Point) -> BTreeSet<(i32, i32)> {
        let reference = fill_barycentric(a, b, c, C);
        let set = positions(&reference);
        assert_eq!(set.len(), reference.len(), "baricéntrico repite fragmentos en {a:?} {b:?} {c:?}");
        for (p, q, r) in [(a, b, c), (a, c, b), (b, a, c), (b, c, a), (c, a, b), (c, b, a)] {
            let bary = fill_barycentric(p, q, r, C);
            let scan = fill_scanline(p, q, r, C);
            assert_eq!(positions(&bary), set, "baricéntrico cambia con el orden {p:?} {q:?} {r:?}");
            assert_eq!(positions(&scan), set, "scanline difiere en {p:?} {q:?} {r:?}");
            assert_eq!(scan.len(), set.len(), "scanline repite fragmentos en {p:?} {q:?} {r:?}");
        }
        set
    }

    fn twice_area(a: Point, b: Point, c: Point) -> i64 {
        ((b.0 - a.0) as i64 * (c.1 - a.1) as i64 - (b.1 - a.1) as i64 * (c.0 - a.0) as i64).abs()
    }

    #[test]
    fn triangulo_pequeno() {
        let s = check_same((2, 2), (6, 3), (3, 7));
        assert!(!s.is_empty());
    }

    #[test]
    fn triangulo_grande() {
        let (a, b, c) = ((10, 10), (500, 40), (120, 400));
        let s = check_same(a, b, c);
        let area = twice_area(a, b, c) as f64 / 2.0;
        assert!((s.len() as f64 - area).abs() < 0.01 * area, "{} vs {area}", s.len());
    }

    #[test]
    fn borde_horizontal() {
        let s = check_same((10, 10), (60, 10), (35, 50));
        assert!(s.iter().all(|&(_, y)| (10..50).contains(&y)));
        assert!(s.iter().any(|&(_, y)| y == 10), "la fila del borde superior debe dibujarse");
    }

    #[test]
    fn borde_horizontal_abajo() {
        let s = check_same((10, 50), (60, 50), (35, 10));
        assert!(s.iter().all(|&(_, y)| (10..50).contains(&y)));
        assert!(!s.iter().any(|&(_, y)| y == 50), "el borde inferior no se dibuja");
    }

    #[test]
    fn borde_vertical() {
        let s = check_same((10, 10), (10, 60), (50, 35));
        assert!(s.iter().all(|&(x, _)| (10..50).contains(&x)));
        assert!(s.iter().any(|&(x, _)| x == 10), "el borde izquierdo se dibuja");
    }

    #[test]
    fn borde_vertical_derecho() {
        let s = check_same((50, 10), (50, 60), (10, 35));
        assert!(s.iter().all(|&(x, _)| (10..50).contains(&x)));
        assert!(!s.iter().any(|&(x, _)| x == 50), "el borde derecho no se dibuja");
    }

    #[test]
    fn orden_inverso_no_desaparece() {
        let (a, b, c) = ((5, 5), (40, 12), (18, 33));
        let adelante = fill_barycentric(a, b, c, C);
        let atras = fill_barycentric(c, b, a, C);
        assert!(!adelante.is_empty() && !atras.is_empty());
        assert_eq!(positions(&adelante), positions(&atras));
        assert_eq!(positions(&fill_scanline(a, b, c, C)), positions(&fill_scanline(c, b, a, C)));
    }

    #[test]
    fn parcialmente_fuera_del_framebuffer() {
        let s = check_same((-20, -10), (30, 5), (5, 40));
        assert!(s.iter().any(|&(x, y)| x < 0 || y < 0), "hay fragmentos fuera por arriba/izquierda");
        let dentro = s.iter().filter(|&&(x, y)| (0..16).contains(&x) && (0..16).contains(&y)).count();
        assert!(dentro > 0);
        let s2 = check_same((60, 60), (200, 70), (90, 150));
        assert!(s2.iter().any(|&(x, y)| x >= 100 || y >= 100), "hay fragmentos fuera por abajo/derecha");
    }

    #[test]
    fn vertices_colineales() {
        for (a, b, c) in [
            ((0, 0), (5, 5), (10, 10)),
            ((0, 0), (10, 0), (20, 0)),
            ((3, 1), (3, 9), (3, 4)),
            ((0, 0), (4, 2), (8, 4)),
            ((3, 3), (3, 3), (9, 2)), // vértice repetido
            ((7, 7), (7, 7), (7, 7)),
        ] {
            assert!(fill_barycentric(a, b, c, C).is_empty(), "{a:?} {b:?} {c:?}");
            assert!(fill_scanline(a, b, c, C).is_empty(), "{a:?} {b:?} {c:?}");
        }
    }

    #[test]
    fn borde_compartido_sin_huecos_ni_duplicados() {
        // Un rectángulo (0,0)-(w,h) partido por cualquiera de sus dos diagonales: la unión debe
        // cubrir exactamente w*h píxeles, cada uno una sola vez.
        let (w, h) = (23, 17);
        let (p, q, r, s) = ((0, 0), (w, 0), (w, h), (0, h));
        for fill in [fill_barycentric, fill_scanline] {
            for (t1, t2) in [([p, q, r], [p, r, s]), ([p, q, s], [q, r, s])] {
                let mut todos = fill(t1[0], t1[1], t1[2], C);
                todos.extend(fill(t2[0], t2[1], t2[2], C));
                assert_eq!(todos.len(), (w * h) as usize);
                assert_eq!(positions(&todos).len(), (w * h) as usize);
            }
        }
        // Cuadrilátero irregular partido por una diagonal con pendiente: sin huecos ni solapes.
        let (p, q, r, s) = ((3, 2), (41, 9), (33, 38), (-4, 25));
        for fill in [fill_barycentric, fill_scanline] {
            let mut a = fill(p, q, r, C);
            let b = fill(p, r, s, C);
            let (na, nb) = (positions(&a), positions(&b));
            assert!(na.is_disjoint(&nb), "fragmentos repetidos en la diagonal");
            a.extend(b);
            let union: BTreeSet<_> = na.union(&nb).copied().collect();
            // El cuadrilátero entero (otro orden de partición) debe dar la misma unión.
            let mut alt = fill(q, r, s, C);
            alt.extend(fill(q, s, p, C));
            assert_eq!(union, positions(&alt));
        }
    }

    /// Generador congruencial simple y determinista: no hace falta un crate externo.
    fn lcg(seed: &mut u64) -> i32 {
        *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((*seed >> 33) % 121) as i32 - 30 // [-30, 90]
    }

    #[test]
    fn triangulos_aleatorios_coinciden() {
        let mut seed = 0x2545F4914F6CDD1D;
        let mut con_fragmentos = 0;
        for _ in 0..3000 {
            let (a, b, c) = ((lcg(&mut seed), lcg(&mut seed)), (lcg(&mut seed), lcg(&mut seed)), (lcg(&mut seed), lcg(&mut seed)));
            let s = check_same(a, b, c);
            if !s.is_empty() {
                con_fragmentos += 1;
            }
            // El número de centros dentro de un triángulo se parece a su área (error ~ perímetro).
            let area = twice_area(a, b, c) as f64 / 2.0;
            let per = [(a, b), (b, c), (c, a)].iter().map(|(p, q)| ((q.0 - p.0).abs() + (q.1 - p.1).abs()) as f64).sum::<f64>();
            assert!((s.len() as f64 - area).abs() <= per, "{a:?} {b:?} {c:?}: {} vs {area}", s.len());
        }
        assert!(con_fragmentos > 2000);
    }

    #[test]
    fn wireframe_incluye_vertices_y_usa_el_color() {
        let (a, b, c) = ((2, 2), (12, 4), (5, 14));
        let frags = wireframe(a, b, c, [1, 2, 3, 255]);
        let s = positions(&frags);
        assert!(s.contains(&a) && s.contains(&b) && s.contains(&c));
        assert!(frags.iter().all(|f| f.color == [1, 2, 3, 255]));
    }

    #[test]
    fn rasterize_conserva_el_orden() {
        let t1 = Triangle { v: [(0, 0), (10, 0), (0, 10)], color: [1, 0, 0, 255] };
        let t2 = Triangle { v: [(0, 0), (10, 0), (0, 10)], color: [2, 0, 0, 255] };
        let f = rasterize(&[t1, t2], TriangleMode::FilledScanline);
        let n = f.len() / 2;
        assert!(f[..n].iter().all(|p| p.color[0] == 1) && f[n..].iter().all(|p| p.color[0] == 2));
    }
}