//! Input model: stable field ids, immediate-mode hit targets and their map.

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FieldId {
	Username,
	Password,
	Cookie,
	Code,
	/// Filter field of the friends list (Home tab).
	Search,
	/// Search field of the Add Friends pane.
	AddSearch,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Hit {
	None,
	Field(FieldId),
	/// The little × inside a text field (clears it, hides results).
	ClearField(FieldId),
	TabPassword,
	TabCookie,
	SignIn,
	Verify,
	Resend,
	Back,
	/// The sidebar logo / wordmark: go Home, refresh, scroll to top.
	Logo,
	/// Sidebar navigation.
	NavFriends,
	NavSettings,
	/// Login screen: abort "add another account" and return to the app.
	CancelAdd,
	Logout,
	/// Collapse / expand the sidebar.
	SidebarToggle,
	/// Scrollbar parts of vertical lists (Roblox-style arrows + track).
	ScrollUp,
	ScrollDown,
	Scrollbar,
	/// Left/right arrows of the horizontal friends strip.
	StripLeft,
	StripRight,
	/// "Search" button of the places search bar (Home pane).
	SearchPlacesBtn,
	/// "Join" button on a place card (place id; joining is not implemented
	/// yet — visual only for now).
	JoinPlace(u64),
	/// "Search" button in the Find-friends pane.
	AddSearchBtn,
	/// "Add" button on a search-result row (user id).
	AddUser(u64),
	/// Friends pane tabs.
	TabRequests,
	TabSearch,
	/// Accept / decline an incoming friend request (user id).
	RequestAccept(u64),
	RequestDecline(u64),
	/// Quick-switch to another saved account (user id).
	SwitchAccount(u64),
	/// Settings: open the login screen to add another account.
	AddAccount,
	/// Settings: theme preset row (index into PRESETS + custom).
	ThemePreset(u8),
	/// Settings: recommendations on/off toggle.
	ToggleRecommendations,
	/// Appearance tab: settings sub-tabs (0 = general, 1 = appearance).
	SettingsTab(u8),
	/// Appearance tab: global corner radius +/-.
	RadiusUp,
	RadiusDown,
	/// Appearance tab: cycle color slot i through the palette.
	ColorPrev(usize),
	ColorNext(usize),
	/// Appearance tab: restore default (dark) colors.
	ResetAppearance,
	/// Settings → Logs: copy the whole diagnostic log buffer to the clipboard.
	CopyLogs,
	/// Settings → Logs: save the persistent log file into the user-visible
	/// Downloads location (Android MediaStore) — the no-root way to send a
	/// full crash report.
	SaveLogs,
	/// Settings → Logs: wipe the diagnostic log buffer.
	ClearLogs,
}

#[derive(Clone, Copy, Debug)]
pub struct Rect {
	pub x: i32,
	pub y: i32,
	pub w: i32,
	pub h: i32,
}

impl Rect {
	pub fn contains(&self, x: f32, y: f32) -> bool {
		x >= self.x as f32
			&& x < (self.x + self.w) as f32
			&& y >= self.y as f32
			&& y < (self.y + self.h) as f32
	}
}

#[derive(Default)]
pub struct HitMap {
	rects: Vec<(Rect, Hit)>,
}

impl HitMap {
	pub fn clear(&mut self) {
		self.rects.clear();
	}
	pub fn add(&mut self, r: Rect, h: Hit) {
		self.rects.push((r, h));
	}
	pub fn at(&self, x: f32, y: f32) -> Hit {
		for (r, h) in self.rects.iter().rev() {
			if r.contains(x, y) {
				return h.clone();
			}
		}
		Hit::None
	}
	pub fn rect_of(&self, target: &Hit) -> Option<Rect> {
		self.rects.iter().find(|(_, h)| h == target).map(|(r, _)| *r)
	}
}
