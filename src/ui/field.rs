//! Retained single-line text field: value, caret, selection and editing
//! operations (UTF-8 boundary safe). Drawing lives in `widgets.rs`.

pub struct TextField {
	pub value: String,
	/// Byte offset of the caret inside `value`.
	pub cursor: usize,
	/// Selected byte range (start <= end).
	pub sel: Option<(usize, usize)>,
	pub masked: bool,
	pub numeric: bool,
	pub max_bytes: usize,
	pub placeholder: &'static str,
	/// Horizontal scroll offset in px (for long values like cookies).
	pub scroll: f32,
}

impl TextField {
	pub fn new(placeholder: &'static str) -> Self {
		TextField {
			value: String::new(),
			cursor: 0,
			sel: None,
			masked: false,
			numeric: false,
			max_bytes: 4096,
			placeholder,
			scroll: 0.0,
		}
	}

	pub fn masked(mut self) -> Self {
		self.masked = true;
		self
	}

	pub fn numeric(mut self) -> Self {
		self.numeric = true;
		self
	}

	pub fn clear(&mut self) {
		self.value.clear();
		self.cursor = 0;
		self.sel = None;
		self.scroll = 0.0;
	}

	/// Replaces the whole value (used by the platform IME sync on mobile).
	pub fn set_text(&mut self, s: &str) {
		let mut v = s.to_string();
		while v.len() > self.max_bytes {
			v.pop();
		}
		self.value = v;
		self.cursor = self.value.len();
		self.sel = None;
		self.scroll = 0.0;
	}

	fn clamp_cursor(&mut self) {
		let mut c = self.cursor.min(self.value.len());
		while c > 0 && !self.value.is_char_boundary(c) {
			c -= 1;
		}
		self.cursor = c;
	}

	pub fn insert(&mut self, s: &str) {
		self.delete_selection();
		let filtered: String = if self.numeric {
			s.chars().filter(|c| c.is_ascii_digit()).collect()
		} else {
			s.chars().filter(|c| *c != '\n' && *c != '\r' && *c != '\t').collect()
		};
		if self.value.len() + filtered.len() > self.max_bytes {
			return;
		}
		if filtered.is_empty() {
			return;
		}
		self.clamp_cursor();
		self.value.insert_str(self.cursor, &filtered);
		self.cursor += filtered.len();
		self.clamp_cursor();
	}

	pub fn delete_selection(&mut self) -> bool {
		if let Some((a, b)) = self.sel.take() {
			let (a, b) = (a.min(b), a.max(b));
			if a <= self.value.len() && b <= self.value.len() {
				self.value.replace_range(a..b, "");
				self.cursor = a;
				self.clamp_cursor();
				return true;
			}
		}
		false
	}

	pub fn backspace(&mut self) {
		if self.delete_selection() {
			return;
		}
		self.clamp_cursor();
		if self.cursor > 0 {
			let mut start = self.cursor - 1;
			while start > 0 && !self.value.is_char_boundary(start) {
				start -= 1;
			}
			self.value.replace_range(start..self.cursor, "");
			self.cursor = start;
		}
	}

	pub fn delete(&mut self) {
		if self.delete_selection() {
			return;
		}
		self.clamp_cursor();
		if self.cursor < self.value.len() {
			let mut end = self.cursor + 1;
			while end < self.value.len() && !self.value.is_char_boundary(end) {
				end += 1;
			}
			self.value.replace_range(self.cursor..end, "");
		}
	}

	pub fn left(&mut self) {
		if self.sel.is_some() {
			self.sel = None;
			self.clamp_cursor();
			return;
		}
		self.clamp_cursor();
		if self.cursor > 0 {
			self.cursor -= 1;
			self.clamp_cursor();
		}
	}

	pub fn right(&mut self) {
		if self.sel.is_some() {
			self.sel = None;
			self.clamp_cursor();
			return;
		}
		self.clamp_cursor();
		if self.cursor < self.value.len() {
			self.cursor += 1;
			self.clamp_cursor();
		}
	}

	pub fn home(&mut self) {
		self.sel = None;
		self.cursor = 0;
	}

	pub fn end(&mut self) {
		self.sel = None;
		self.cursor = self.value.len();
	}

	pub fn select_all(&mut self) {
		if !self.value.is_empty() {
			self.sel = Some((0, self.value.len()));
			self.cursor = self.value.len();
		}
	}

	/// Click inside the field: place caret near byte offset `click_bytes`.
	pub fn click_at(&mut self, click_bytes: usize) {
		// Convert byte estimate to a char boundary (approximation is fine).
		let mut n = click_bytes.min(self.value.len());
		while n > 0 && !self.value.is_char_boundary(n) {
			n -= 1;
		}
		self.cursor = n;
		self.sel = None;
	}
}
