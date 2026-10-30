// Package server exposes the engine over a local HTTP JSON API with SSE events.
package server

import (
	"encoding/json"
	"errors"
	"fmt"
	"io/fs"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"sort"
	"strconv"
	"strings"
	"time"

	"modlisttool/internal/engine"
)

// Config persists the last opened instance next to the executable.
type Config struct {
	Instance string   `json:"instance"`
	Extra    []string `json:"extra"`
	Profile  string   `json:"profile"`
	Recent   []string `json:"recent"`
}

// Server wires the engine to HTTP.
type Server struct {
	E       *engine.Engine
	Static  fs.FS
	CfgPath string
	Cfg     Config
	Version string
}

// LoadConfig reads the config file (missing is fine).
func (s *Server) LoadConfig() {
	if data, err := os.ReadFile(s.CfgPath); err == nil {
		_ = json.Unmarshal(data, &s.Cfg)
	}
}

func (s *Server) saveConfig() {
	data, _ := json.MarshalIndent(s.Cfg, "", " ")
	_ = os.WriteFile(s.CfgPath, data, 0o644)
}

// Handler returns the HTTP handler.
func (s *Server) Handler() http.Handler {
	mux := http.NewServeMux()
	api := func(path string, fn func(r *http.Request) (any, error)) {
		mux.HandleFunc(path, func(w http.ResponseWriter, r *http.Request) {
			defer func() {
				if rec := recover(); rec != nil {
					writeJSON(w, 500, map[string]any{"error": fmt.Sprint(rec)})
				}
			}()
			v, err := fn(r)
			if err != nil {
				writeJSON(w, 400, map[string]any{"error": err.Error()})
				return
			}
			writeJSON(w, 200, v)
		})
	}
	api("GET /api/status", func(r *http.Request) (any, error) {
		return map[string]any{"version": s.Version, "config": s.Cfg, "opened": s.E.Inst != nil, "run": s.E.RunState(), "os": runtime.GOOS}, nil
	})
	api("POST /api/open", func(r *http.Request) (any, error) {
		var req struct {
			Path    string   `json:"path"`
			Extra   []string `json:"extra"`
			Profile string   `json:"profile"`
		}
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			return nil, err
		}
		if err := s.E.Open(req.Path, req.Extra, req.Profile); err != nil {
			return nil, err
		}
		s.Cfg.Instance, s.Cfg.Extra, s.Cfg.Profile = req.Path, req.Extra, s.E.Inst.Profile
		s.Cfg.Recent = addRecent(s.Cfg.Recent, req.Path)
		s.saveConfig()
		return s.E.Modlist(), nil
	})
	api("GET /api/modlist", func(r *http.Request) (any, error) { return s.E.Modlist(), nil })
	api("GET /api/summary", func(r *http.Request) (any, error) { return s.E.Summary(), nil })
	api("GET /api/journal", func(r *http.Request) (any, error) { return s.E.Journal(), nil })
	api("GET /api/tags", func(r *http.Request) (any, error) { return s.E.AllTags(), nil })
	api("GET /api/plugins", func(r *http.Request) (any, error) { return s.E.Plugins(), nil })
	api("GET /api/run", func(r *http.Request) (any, error) { return s.E.RunState(), nil })
	api("POST /api/scan", func(r *http.Request) (any, error) {
		opt := engine.DefaultScanOptions()
		if err := json.NewDecoder(r.Body).Decode(&opt); err != nil {
			return nil, err
		}
		return map[string]bool{"ok": true}, s.E.StartScan(opt)
	})
	api("POST /api/move", func(r *http.Request) (any, error) {
		var req struct {
			Names  []string `json:"names"`
			Before string   `json:"before"`
		}
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			return nil, err
		}
		return ok(), s.E.Move(req.Names, req.Before)
	})
	api("POST /api/toggle", func(r *http.Request) (any, error) {
		var req struct {
			Names   []string `json:"names"`
			Enabled *bool    `json:"enabled"`
		}
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			return nil, err
		}
		return ok(), s.E.SetEnabled(req.Names, req.Enabled)
	})
	api("POST /api/undo", func(r *http.Request) (any, error) { return map[string]bool{"ok": s.E.Undo()}, nil })
	api("POST /api/redo", func(r *http.Request) (any, error) { return map[string]bool{"ok": s.E.Redo()}, nil })
	api("POST /api/save-modlist", func(r *http.Request) (any, error) {
		v, err := s.E.SaveModlist()
		return map[string]string{"backup": v}, err
	})
	api("POST /api/user", func(r *http.Request) (any, error) {
		var p engine.UserPatch
		if err := json.NewDecoder(r.Body).Decode(&p); err != nil {
			return nil, err
		}
		return ok(), s.E.ApplyUser(p)
	})
	api("GET /api/mod", func(r *http.Request) (any, error) { return s.E.Mod(r.URL.Query().Get("name")) })
	api("GET /api/mod/chains", func(r *http.Request) (any, error) { return s.E.ModChains(r.URL.Query().Get("name")) })
	api("GET /api/mod/tree", func(r *http.Request) (any, error) {
		q := r.URL.Query()
		return s.E.Tree(q.Get("name"), q.Get("dir"))
	})
	api("GET /api/chain/files", func(r *http.Request) (any, error) {
		q := r.URL.Query()
		id, _ := strconv.Atoi(q.Get("id"))
		off, _ := strconv.Atoi(q.Get("offset"))
		lim, _ := strconv.Atoi(q.Get("limit"))
		return s.E.ChainFiles(int32(id), q.Get("q"), off, lim)
	})
	api("GET /api/contexts", func(r *http.Request) (any, error) { return s.E.Contexts(), nil })
	api("GET /api/context", func(r *http.Request) (any, error) {
		id, _ := strconv.Atoi(r.URL.Query().Get("id"))
		return s.E.Context(int32(id))
	})
	api("GET /api/search", func(r *http.Request) (any, error) {
		q := r.URL.Query()
		off, _ := strconv.Atoi(q.Get("offset"))
		lim, _ := strconv.Atoi(q.Get("limit"))
		return s.E.SearchFiles(q.Get("q"), off, lim)
	})
	api("POST /api/reveal", func(r *http.Request) (any, error) {
		var req struct {
			Name string `json:"name"`
		}
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			return nil, err
		}
		if s.E.Inst == nil {
			return nil, errors.New("профиль не открыт")
		}
		return ok(), reveal(s.E.ResolveModDir(req.Name))
	})
	api("GET /api/fs/list", func(r *http.Request) (any, error) { return listDir(r.URL.Query().Get("path")) })
	mux.HandleFunc("GET /api/events", s.events)
	if s.Static != nil {
		files := http.FileServer(http.FS(s.Static))
		mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
			if r.URL.Path != "/" {
				if _, err := fs.Stat(s.Static, strings.TrimPrefix(r.URL.Path, "/")); err != nil {
					r.URL.Path = "/"
				}
			}
			w.Header().Set("Cache-Control", "no-cache")
			files.ServeHTTP(w, r)
		})
	}
	return mux
}

func ok() map[string]bool { return map[string]bool{"ok": true} }

func writeJSON(w http.ResponseWriter, code int, v any) {
	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	w.WriteHeader(code)
	enc := json.NewEncoder(w)
	_ = enc.Encode(v)
}

func (s *Server) events(w http.ResponseWriter, r *http.Request) {
	fl, okf := w.(http.Flusher)
	if !okf {
		http.Error(w, "no flush", 500)
		return
	}
	w.Header().Set("Content-Type", "text/event-stream")
	w.Header().Set("Cache-Control", "no-cache")
	ch, cancel := s.E.Subscribe()
	defer cancel()
	ping := time.NewTicker(20 * time.Second)
	defer ping.Stop()
	fmt.Fprint(w, ": hi\n\n")
	fl.Flush()
	for {
		select {
		case <-r.Context().Done():
			return
		case <-ping.C:
			fmt.Fprint(w, ": ping\n\n")
			fl.Flush()
		case ev := <-ch:
			data, _ := json.Marshal(ev)
			fmt.Fprintf(w, "data: %s\n\n", data)
			fl.Flush()
		}
	}
}

func addRecent(list []string, p string) []string {
	out := []string{p}
	for _, x := range list {
		if x != p && len(out) < 8 {
			out = append(out, x)
		}
	}
	return out
}

type fsEntry struct {
	Name  string `json:"name"`
	Path  string `json:"path"`
	Dir   bool   `json:"dir"`
	IsMO2 bool   `json:"isMO2,omitempty"`
}

func listDir(p string) (any, error) {
	if p == "" {
		if runtime.GOOS == "windows" {
			var out []fsEntry
			for c := 'A'; c <= 'Z'; c++ {
				d := string(c) + `:\`
				if _, err := os.Stat(d); err == nil {
					out = append(out, fsEntry{Name: d, Path: d, Dir: true})
				}
			}
			return map[string]any{"path": "", "parent": "", "entries": out}, nil
		}
		p = "/"
	}
	ents, err := os.ReadDir(p)
	if err != nil {
		return nil, err
	}
	var out []fsEntry
	for _, e := range ents {
		if !e.IsDir() && !strings.EqualFold(e.Name(), "ModOrganizer.ini") {
			continue
		}
		fp := filepath.Join(p, e.Name())
		fe := fsEntry{Name: e.Name(), Path: fp, Dir: e.IsDir()}
		if e.IsDir() {
			if _, err := os.Stat(filepath.Join(fp, "ModOrganizer.ini")); err == nil {
				fe.IsMO2 = true
			}
		}
		out = append(out, fe)
	}
	sort.Slice(out, func(a, b int) bool { return strings.ToLower(out[a].Name) < strings.ToLower(out[b].Name) })
	parent := filepath.Dir(filepath.Clean(p))
	if parent == filepath.Clean(p) {
		parent = ""
	}
	return map[string]any{"path": p, "parent": parent, "entries": out}, nil
}

func reveal(p string) error {
	switch runtime.GOOS {
	case "windows":
		return exec.Command("explorer", p).Start()
	case "darwin":
		return exec.Command("open", p).Start()
	}
	return exec.Command("xdg-open", p).Start()
}

// OpenBrowser opens the UI URL.
func OpenBrowser(url string) {
	switch runtime.GOOS {
	case "windows":
		_ = exec.Command("rundll32", "url.dll,FileProtocolHandler", url).Start()
	case "darwin":
		_ = exec.Command("open", url).Start()
	default:
		_ = exec.Command("xdg-open", url).Start()
	}
}
