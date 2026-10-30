package engine

import (
	"sort"
	"strings"

	"modlisttool/internal/formats"
)

// PluginView is one plugin in the profile.
type PluginView struct {
	Name           string   `json:"name"`
	Enabled        bool     `json:"enabled"`
	Index          int      `json:"index"`          // position in load order (all plugins)
	Slot           string   `json:"slot,omitempty"` // xx / FE:xxx for active plugins
	Kind           string   `json:"kind"`
	Masters        []string `json:"masters,omitempty"`
	MissingMasters []string `json:"missingMasters,omitempty"`
	LateMasters    []string `json:"lateMasters,omitempty"` // masters loaded after this plugin
	Owner          string   `json:"owner,omitempty"`
	Owners         []string `json:"owners,omitempty"`
	Found          bool     `json:"found"`
	InLoadorder    bool     `json:"inLoadorder"`
	Records        uint32   `json:"records,omitempty"`
	Refs           uint32   `json:"refs,omitempty"`
	Author         string   `json:"author,omitempty"`
	Error          string   `json:"error,omitempty"`
}

var baseGame = map[string]bool{"skyrim.esm": true, "update.esm": true, "dawnguard.esm": true, "hearthfires.esm": true, "dragonborn.esm": true}

// Plugins returns plugins in load order with scan data.
func (e *Engine) Plugins() []PluginView {
	e.mu.RLock()
	defer e.mu.RUnlock()
	return e.pluginsLocked()
}

func (e *Engine) pluginsLocked() []PluginView {
	enabled := map[string]bool{}
	for _, p := range e.plugins {
		enabled[strings.ToLower(p.Name)] = p.Enabled
	}
	// scanned headers with owners in priority order (winner last)
	type found struct {
		p      *formats.Plugin
		owners []string
		prio   int32
	}
	scanned := map[string]*found{}
	for id, m := range e.scans {
		if m == nil {
			continue
		}
		pr := int32(-1)
		if e.conf != nil {
			pr = e.conf.Prio[id]
		}
		for _, p := range m.Plugins {
			k := strings.ToLower(p.Name)
			f := scanned[k]
			if f == nil {
				f = &found{prio: -2}
				scanned[k] = f
			}
			f.owners = append(f.owners, m.Name)
			if pr > f.prio {
				f.p, f.prio = p, pr
			}
		}
	}
	order := append([]string(nil), e.loadorder...)
	inLO := map[string]bool{}
	for _, n := range order {
		inLO[strings.ToLower(n)] = true
	}
	for _, p := range e.plugins {
		if !inLO[strings.ToLower(p.Name)] {
			order = append(order, p.Name)
			inLO[strings.ToLower(p.Name)] = true
		}
	}
	var extra []string
	for k, f := range scanned {
		if !inLO[k] && f.p != nil {
			extra = append(extra, f.p.Name)
		}
	}
	sort.Strings(extra)
	pos := map[string]int{}
	out := make([]PluginView, 0, len(order)+len(extra))
	full, light := 0, 0
	for i, n := range append(order, extra...) {
		k := strings.ToLower(n)
		v := PluginView{Name: n, Index: i, InLoadorder: inLO[k] && i < len(order)}
		v.Enabled = enabled[k] || (baseGame[k] && v.InLoadorder)
		if f := scanned[k]; f != nil && f.p != nil {
			v.Found = true
			v.Kind = f.p.Kind()
			v.Masters = f.p.Masters
			v.Records = f.p.NumRecords
			v.Author = f.p.Author
			v.Error = f.p.Error
			v.Owners = f.owners
			v.Owner = f.owners[len(f.owners)-1]
			if f.p.Records != nil {
				v.Refs = f.p.Records["REFR"] + f.p.Records["ACHR"]
			}
		} else {
			v.Kind = strings.ToUpper(strings.TrimPrefix(strings.ToLower(n[max(0, len(n)-4):]), "."))
		}
		if v.Enabled {
			if v.Kind == "ESL" {
				v.Slot = "FE:" + hex3(light)
				light++
			} else {
				v.Slot = hex2(full)
				full++
			}
		}
		pos[k] = i
		out = append(out, v)
	}
	for i := range out {
		for _, m := range out[i].Masters {
			k := strings.ToLower(m)
			mp, ok := pos[k]
			switch {
			case !ok || (!out[mp].Enabled && out[i].Enabled):
				out[i].MissingMasters = append(out[i].MissingMasters, m)
			case mp > i:
				out[i].LateMasters = append(out[i].LateMasters, m)
			}
		}
	}
	return out
}

func hex2(n int) string { return strings.ToUpper(string("0123456789abcdef"[n>>4&15]) + string("0123456789abcdef"[n&15])) }
func hex3(n int) string {
	const h = "0123456789ABCDEF"
	return string(h[n>>8&15]) + string(h[n>>4&15]) + string(h[n&15])
}
