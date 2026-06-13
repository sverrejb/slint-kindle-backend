const BASE: i32 = b'R' as i32;

const TRACKING: f32 = -2.0;

const PAD: f32 = 4.0;

struct Glyph {
    left: f32,
    right: f32,
    strokes: Vec<Vec<(f32, f32)>>,
}

fn parse(jhf: &str) -> Vec<Glyph> {
    let lines: Vec<&str> = jhf.lines().collect();
    let mut glyphs = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
   
        if line.len() < 8 {
            continue;
        }
        let nverts: usize = match line[5..8].trim().parse() {
            Ok(n) => n,
            Err(_) => continue,
        };
     
        let mut data = String::from(&line[8..]);
        while data.len() < nverts * 2 {
            data.push_str(lines[i]);
            i += 1;
        }

        let b = data.as_bytes();
        let left = (b[0] as i32 - BASE) as f32;
        let right = (b[1] as i32 - BASE) as f32;

        let mut strokes = Vec::new();
        let mut current: Vec<(f32, f32)> = Vec::new();
        let mut k = 2;
        while k < nverts * 2 {
            let (c1, c2) = (b[k], b[k + 1]);
            if c1 == b' ' {
                if !current.is_empty() {
                    strokes.push(std::mem::take(&mut current));
                }
            } else {
                let x = (c1 as i32 - BASE) as f32;
                let y = (c2 as i32 - BASE) as f32;
                current.push((x, y));
            }
            k += 2;
        }
        if !current.is_empty() {
            strokes.push(current);
        }

        glyphs.push(Glyph { left, right, strokes });
    }
    glyphs
}


pub fn build_points(jhf: &str, word: &str) -> (Vec<(f32, f32, bool)>, (f32, f32, f32, f32)) {
    let glyphs = parse(jhf);
    let mut pts = Vec::new();
    let mut cursor = 0.0f32;
    for ch in word.chars() {
        let gi = ch as i32 - 32;
        if gi < 0 || gi as usize >= glyphs.len() {
            continue;
        }
        let g = &glyphs[gi as usize];
        for stroke in &g.strokes {
            for (j, &(x, y)) in stroke.iter().enumerate() {
                pts.push((cursor + (x - g.left), y, j == 0));
            }
        }
        cursor += g.right - g.left;
        if ch.is_ascii_uppercase() {
            cursor += TRACKING;
        }
    }

    if pts.is_empty() {
        return (pts, (0.0, 0.0, 0.0, 0.0));
    }

    let (mut min_x, mut min_y) = (f32::MAX, f32::MAX);
    let (mut max_x, mut max_y) = (f32::MIN, f32::MIN);
    for &(x, y, _) in &pts {
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    let viewbox = (
        min_x - PAD,
        min_y - PAD,
        (max_x - min_x) + 2.0 * PAD,
        (max_y - min_y) + 2.0 * PAD,
    );
    (pts, viewbox)
}
