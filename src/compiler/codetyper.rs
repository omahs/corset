use colored::{Color, Colorize};

pub struct Tty {
    depths: Vec<usize>,
    o: Vec<String>,
    latch: usize,
}
impl Tty {
    pub fn new() -> Self {
        Self {
            depths: vec![0],
            o: vec![String::new()],
            latch: 0,
        }
    }

    pub fn write<S: AsRef<str>>(&mut self, l: S) {
        if self.latch > 0 {
            self.latch -= 1;
            self.o.last_mut().unwrap().push_str(l.as_ref());
        } else {
            let indent = self
                .depths
                .iter()
                .skip(1)
                .map(|d| " ".repeat((*d as isize - 1).max(0) as usize))
                .collect::<Vec<_>>()
                .join("│".color(Color::BrightBlack).to_string().as_str());
            self.o
                .last_mut()
                .unwrap()
                .push_str(&format!("{}{}", indent, l.as_ref()));
        }
    }
    pub fn append<S: AsRef<str>>(&mut self, s: S) {
        self.o.last_mut().unwrap().push_str(s.as_ref())
    }
    pub fn shift(&mut self, d: usize) {
        self.depths.push(d);
    }
    pub fn unshift(&mut self) {
        self.depths.pop();
    }
    pub fn cr(&mut self) {
        self.o.push(String::new());
    }
    pub fn page_feed(&self) -> String {
        self.o.join("\n")
    }
    pub fn latch_indent(&mut self) {
        self.latch += 1
    }
    pub fn depth(&self) -> usize {
        self.depths.len()
    }
}