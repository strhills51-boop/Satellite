package engine

import (
	"encoding/json"
	"os"
	"sort"
	"strings"
	"time"

	"modlisttool/internal/mo2"
)

// ModUser is the user's decorative layer for a mod (never written into MO2).
type ModUser struct {
	Tags     []string `json:"tags,omitempty"`
	Stickers []string `json:"stickers,omitempty"`
	Auto     []string `json:"auto,omitempty"` // tags from rules
	Note     string   `json:"note,omitempty"`
	Color    string   `json:"color,omitempty"`
	Emoji    string   `json:"emoji,omitempty"`
	Locked   bool     `json:"locked,omitempty"`
}

// SepUser decorates a separator.
type SepUser struct {
	Color     string `json:"color,omitempty"`
	Emoji     string `json:"emoji,omitempty"`
	Collapsed bool   `json:"collapsed,omitempty"`
}

// ChainUser stores decisions about a chain, keyed by its member set.
type ChainUser struct {
	Done  bool   `json:"done,omitempty"`
	Label string `json:"label,omitempty"`
	Color string `json:"color,omitempty"`
	Icon  string `json:"icon,omitempty"`
}

// JournalEntry is one action in the history.
type JournalEntry struct {
	At   time.Time `json:"at"`
	Type string    `json:"type"`
	Text string    `json:"text"`
}

// UserLayer is persisted as monolith/project.json next to the profile.
type UserLayer struct {
	Schema       int                   `json:"schema"`
	Mods         map[string]*ModUser   `json:"mods"`
	Seps         map[string]*SepUser   `json:"seps"`
	Chains       map[string]*ChainUser `json:"chains"`
	Journal      []JournalEntry        `json:"journal"`
	PendingOrder []mo2.Entry           `json:"pendingOrder,omitempty"`
	PendingBase  string                `json:"pendingBase,omitempty"`
	LastScan     time.Time             `json:"lastScan"`
	ScanOptions  *ScanOptions          `json:"scanOptions,omitempty"`
	UI           map[string]any        `json:"ui,omitempty"`
	path         string
}

// LoadUserLayer reads project.json or returns an empty layer.
func LoadUserLayer(path string) *UserLayer {
	u := &UserLayer{Schema: 1}
	if data, err := os.ReadFile(path); err == nil {
		_ = json.Unmarshal(data, u)
	}
	u.path = path
	if u.Mods == nil {
		u.Mods = map[string]*ModUser{}
	}
	if u.Seps == nil {
		u.Seps = map[string]*SepUser{}
	}
	if u.Chains == nil {
		u.Chains = map[string]*ChainUser{}
	}
	if u.UI == nil {
		u.UI = map[string]any{}
	}
	return u
}

func (e *Engine) saveUser() {
	e.mu.RLock()
	if e.User == nil || e.User.path == "" {
		e.mu.RUnlock()
		return
	}
	data, err := json.MarshalIndent(e.User, "", " ")
	path := e.User.path
	e.mu.RUnlock()
	if err == nil {
		_ = mo2.WriteFileAtomic(path, data)
	}
}

func (e *Engine) journal(typ, text string) {
	e.mu.Lock()
	if e.User != nil {
		e.User.Journal = append([]JournalEntry{{At: time.Now(), Type: typ, Text: text}}, e.User.Journal...)
		if len(e.User.Journal) > 500 {
			e.User.Journal = e.User.Journal[:500]
		}
	}
	e.mu.Unlock()
	e.saveUser()
	e.emit("journal", nil)
}

// Journal returns history entries.
func (e *Engine) Journal() []JournalEntry {
	e.mu.RLock()
	defer e.mu.RUnlock()
	if e.User == nil {
		return nil
	}
	return append([]JournalEntry(nil), e.User.Journal...)
}

// UserPatch is a partial update from the UI.
type UserPatch struct {
	Kind  string          `json:"kind"` // mod | sep | chain | ui
	Names []string        `json:"names"`
	Key   string          `json:"key"`
	Patch json.RawMessage `json:"patch"`
	// tag ops for mods
	AddTags    []string `json:"addTags"`
	RemoveTags []string `json:"removeTags"`
}

// ApplyUser merges a patch into the user layer.
func (e *Engine) ApplyUser(p UserPatch) error {
	e.mu.Lock()
	switch p.Kind {
	case "mod":
		for _, n := range p.Names {
			u := e.User.Mods[n]
			if u == nil {
				u = &ModUser{}
				e.User.Mods[n] = u
			}
			if len(p.Patch) > 0 {
				if err := json.Unmarshal(p.Patch, u); err != nil {
					e.mu.Unlock()
					return err
				}
			}
			u.Tags = mergeTags(u.Tags, p.AddTags, p.RemoveTags)
		}
	case "sep":
		for _, n := range p.Names {
			u := e.User.Seps[n]
			if u == nil {
				u = &SepUser{}
				e.User.Seps[n] = u
			}
			if err := json.Unmarshal(p.Patch, u); err != nil {
				e.mu.Unlock()
				return err
			}
		}
	case "chain":
		u := e.User.Chains[p.Key]
		if u == nil {
			u = &ChainUser{}
			e.User.Chains[p.Key] = u
		}
		if err := json.Unmarshal(p.Patch, u); err != nil {
			e.mu.Unlock()
			return err
		}
	case "ui":
		var m map[string]any
		if err := json.Unmarshal(p.Patch, &m); err != nil {
			e.mu.Unlock()
			return err
		}
		for k, v := range m {
			e.User.UI[k] = v
		}
	}
	e.mu.Unlock()
	go e.saveUser()
	e.emit("changed", "user")
	return nil
}

func mergeTags(cur, add, rm []string) []string {
	set := map[string]bool{}
	var out []string
	drop := map[string]bool{}
	for _, t := range rm {
		drop[strings.TrimSpace(t)] = true
	}
	for _, t := range append(cur, add...) {
		t = strings.TrimSpace(t)
		if t == "" || drop[t] || set[t] {
			continue
		}
		set[t] = true
		out = append(out, t)
	}
	return out
}

// AllTags returns every tag in use with counts.
func (e *Engine) AllTags() map[string]int {
	e.mu.RLock()
	defer e.mu.RUnlock()
	out := map[string]int{}
	for _, u := range e.User.Mods {
		for _, t := range u.Tags {
			out[t]++
		}
		for _, t := range u.Auto {
			out[t]++
		}
	}
	return out
}

// applyRules assigns content tags derived from metrics (the monolith's tag rules, built in).
func (e *Engine) applyRules() int {
	e.mu.Lock()
	defer e.mu.Unlock()
	n := 0
	for _, m := range e.scans {
		if m == nil || m.Missing || m.Name == OverwriteName {
			continue
		}
		mt := m.Metrics
		var tags []string
		add := func(cond bool, t string) {
			if cond {
				tags = append(tags, t)
			}
		}
		add(mt.DDS > 0, "текстуры")
		add(mt.NIF > 0, "меши")
		add(mt.PEX > 0, "скрипты")
		add(mt.HKX > 0, "анимации")
		add(mt.DLL > 0, "SKSE/DLL")
		add(mt.Plugins > 0, "плагин")
		add(mt.Archives > 0, "архивы")
		add(mt.Files == 0 && mt.Plugins == 0, "пустой")
		add(mt.Nested > 0 || mt.Source > 0, "cargo?")
		add(mt.TopDirs["lodsettings"] > 0 || mt.TopDirs["terrain"] > 0, "LOD")
		add(mt.TopDirs["interface"] > 0, "UI")
		add(mt.TopDirs["sound"] > 0 || mt.TopDirs["music"] > 0, "звук")
		sort.Strings(tags)
		u := e.User.Mods[m.Name]
		if u == nil {
			if len(tags) == 0 {
				continue
			}
			u = &ModUser{}
			e.User.Mods[m.Name] = u
		}
		u.Auto = tags
		n += len(tags)
	}
	return n
}
