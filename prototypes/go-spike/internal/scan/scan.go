// Package scan walks mod folders in parallel and produces per-mod file listings.
package scan

import (
	"encoding/gob"
	"errors"
	"fmt"
	"hash/fnv"
	"io/fs"
	"os"
	"path/filepath"
	"runtime"
	"sort"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"modlisttool/internal/formats"
)

// CacheVersion bumps invalidate all cached scans.
const CacheVersion = 3

// File is one loose file inside a directory group.
type File struct {
	Name    string
	Size    int64
	VRAMKiB uint32 // DDS only, 0 = unknown
}

// Dir groups files of one directory (Rel uses forward slashes, "" for mod root).
type Dir struct {
	Rel   string
	Files []File
}

// Archive is a parsed BSA/BA2 at the mod root.
type Archive struct {
	Name    string
	Kind    string
	Size    int64
	Entries []formats.ArchiveEntry
	Error   string
}

// Metrics are cheap per-mod aggregates computed during the walk.
type Metrics struct {
	Bytes       int64            `json:"bytes"`
	Files       int              `json:"files"`
	Dirs        int              `json:"dirs"`
	RootFiles   int              `json:"rootFiles"`
	MaxDepth    int              `json:"maxDepth"`
	DDS         int              `json:"dds"`
	DDSKnown    int              `json:"ddsKnown"`
	VRAMBytes   int64            `json:"vramBytes"`
	NIF         int              `json:"nif"`
	PEX         int              `json:"pex"`
	HKX         int              `json:"hkx"`
	DLL         int              `json:"dll"`
	Plugins     int              `json:"plugins"`
	Archives    int              `json:"archives"`
	ArchiveFile int              `json:"archiveFiles"`
	Nested      int              `json:"nestedArchives"` // zip/7z/rar inside mod
	Source      int              `json:"sourceFiles"`    // psc, psd, xcf, blend...
	Ext         map[string]int   `json:"ext"`
	TopDirs     map[string]int   `json:"topDirs"`
	DDSFormats  map[string]int   `json:"ddsFormats,omitempty"`
}

// Mod is the complete scan result for one mod folder.
type Mod struct {
	Version   int
	Name      string
	Path      string
	Signature uint64
	ScannedAt time.Time
	Dirs      []Dir
	Archives  []Archive
	Plugins   []*formats.Plugin
	Metrics   Metrics
	Missing   bool
	Error     string
	FromCache bool
}

// Job describes a folder to scan.
type Job struct {
	Name string
	Path string
}

// Options control the walk.
type Options struct {
	Workers     int
	CacheDir    string
	Force       bool
	ReadDDS     bool
	Archives    bool
	Plugins     bool
	RecordScan  bool
	IgnoreNames []string // lowercased file names to skip (meta.ini …)
}

// Progress is reported via callback.
type Progress struct {
	Done, Total, Cached int
	Files               int64
	Current             string
}

var (
	pluginExt  = map[string]bool{".esp": true, ".esm": true, ".esl": true}
	archiveExt = map[string]bool{".bsa": true, ".ba2": true}
	nestedExt  = map[string]bool{".zip": true, ".7z": true, ".rar": true, ".tar": true, ".gz": true}
	sourceExt  = map[string]bool{".psc": true, ".psd": true, ".xcf": true, ".blend": true, ".max": true, ".fbx": true, ".obj": true, ".spp": true, ".sbsar": true}
)

// Run scans all jobs with a worker pool. Results are returned in job order.
func Run(jobs []Job, opt Options, onProgress func(Progress)) []*Mod {
	if opt.Workers <= 0 {
		opt.Workers = min(32, max(4, runtime.NumCPU()*2))
	}
	ignore := map[string]bool{}
	for _, n := range opt.IgnoreNames {
		ignore[strings.ToLower(n)] = true
	}
	out := make([]*Mod, len(jobs))
	order := make([]int, len(jobs))
	for i := range order {
		order[i] = i
	}
	// Biggest previously-seen mods first so the tail is short.
	sizes := make([]int64, len(jobs))
	if opt.CacheDir != "" {
		for i, j := range jobs {
			if st, err := os.Stat(cachePath(opt.CacheDir, j.Name)); err == nil {
				sizes[i] = st.Size()
			}
		}
	}
	sort.SliceStable(order, func(a, b int) bool { return sizes[order[a]] > sizes[order[b]] })

	var done, cached atomic.Int64
	var files atomic.Int64
	var cur atomic.Value
	cur.Store("")
	ch := make(chan int)
	var wg sync.WaitGroup
	stop := make(chan struct{})
	if onProgress != nil {
		go func() {
			t := time.NewTicker(150 * time.Millisecond)
			defer t.Stop()
			for {
				select {
				case <-stop:
					return
				case <-t.C:
					onProgress(Progress{Done: int(done.Load()), Total: len(jobs), Cached: int(cached.Load()), Files: files.Load(), Current: cur.Load().(string)})
				}
			}
		}()
	}
	for w := 0; w < opt.Workers; w++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for i := range ch {
				j := jobs[i]
				cur.Store(j.Name)
				m := scanOne(j, opt, ignore, &files)
				if m.FromCache {
					cached.Add(1)
					files.Add(int64(m.Metrics.Files))
				}
				out[i] = m
				done.Add(1)
			}
		}()
	}
	for _, i := range order {
		ch <- i
	}
	close(ch)
	wg.Wait()
	close(stop)
	if onProgress != nil {
		onProgress(Progress{Done: len(jobs), Total: len(jobs), Cached: int(cached.Load()), Files: files.Load()})
	}
	return out
}

func scanOne(j Job, opt Options, ignore map[string]bool, files *atomic.Int64) *Mod {
	m := &Mod{Version: CacheVersion, Name: j.Name, Path: j.Path}
	st, err := os.Stat(j.Path)
	if err != nil || !st.IsDir() {
		m.Missing = true
		return m
	}
	sig := Signature(j.Path, opt)
	if !opt.Force && opt.CacheDir != "" {
		if c, err := loadCache(opt.CacheDir, j.Name); err == nil && c.Signature == sig && c.Version == CacheVersion && c.Path == j.Path {
			c.FromCache = true
			return c
		}
	}
	m.Signature = sig
	m.ScannedAt = time.Now()
	m.Metrics.Ext = map[string]int{}
	m.Metrics.TopDirs = map[string]int{}
	if err := walk(m, j.Path, "", 0, opt, ignore, files); err != nil {
		m.Error = err.Error()
	}
	if opt.CacheDir != "" && m.Error == "" {
		_ = saveCache(opt.CacheDir, m)
	}
	return m
}

func walk(m *Mod, abs, rel string, depth int, opt Options, ignore map[string]bool, files *atomic.Int64) error {
	ents, err := os.ReadDir(abs)
	if err != nil {
		return err
	}
	d := Dir{Rel: rel}
	var subdirs []fs.DirEntry
	for _, e := range ents {
		name := e.Name()
		if e.IsDir() {
			subdirs = append(subdirs, e)
			continue
		}
		lname := strings.ToLower(name)
		if depth == 0 && ignore[lname] {
			continue
		}
		info, err := e.Info()
		if err != nil {
			continue
		}
		ext := filepath.Ext(lname)
		f := File{Name: name, Size: info.Size()}
		mt := &m.Metrics
		mt.Files++
		mt.Bytes += f.Size
		mt.Ext[ext]++
		if depth == 0 {
			mt.RootFiles++
		} else {
			top := strings.ToLower(strings.SplitN(rel, "/", 2)[0])
			mt.TopDirs[top]++
		}
		mt.MaxDepth = max(mt.MaxDepth, depth)
		switch {
		case ext == ".dds":
			mt.DDS++
			if opt.ReadDDS {
				dd := formats.ReadDDSHeader(filepath.Join(abs, name))
				if dd.Known {
					f.VRAMKiB = uint32(min(dd.VRAM/1024+1, 1<<32-1))
					mt.DDSKnown++
					mt.VRAMBytes += int64(dd.VRAM)
					if mt.DDSFormats == nil {
						mt.DDSFormats = map[string]int{}
					}
					mt.DDSFormats[dd.Format]++
				}
			}
		case ext == ".nif":
			mt.NIF++
		case ext == ".pex":
			mt.PEX++
		case ext == ".hkx":
			mt.HKX++
		case ext == ".dll":
			mt.DLL++
		case nestedExt[ext]:
			mt.Nested++
		case sourceExt[ext]:
			mt.Source++
		}
		if depth == 0 && pluginExt[ext] {
			mt.Plugins++
			if opt.Plugins {
				p := formats.ReadPluginHeader(filepath.Join(abs, name), name)
				if opt.RecordScan && p.Error == "" {
					_ = formats.ScanRecords(filepath.Join(abs, name), p)
				}
				m.Plugins = append(m.Plugins, p)
			}
		}
		if depth == 0 && archiveExt[ext] {
			mt.Archives++
			if opt.Archives {
				a := Archive{Name: name, Size: f.Size}
				kind, entries, err := formats.ReadArchive(filepath.Join(abs, name))
				a.Kind, a.Entries = kind, entries
				if err != nil {
					a.Error = err.Error()
				}
				mt.ArchiveFile += len(entries)
				for _, e := range entries {
					if strings.HasSuffix(e.Path, ".dds") && e.VRAM > 0 {
						mt.VRAMBytes += int64(e.VRAM)
						mt.DDSKnown++
					}
				}
				m.Archives = append(m.Archives, a)
			}
		}
		d.Files = append(d.Files, f)
	}
	files.Add(int64(len(d.Files)))
	if len(d.Files) > 0 {
		m.Dirs = append(m.Dirs, d)
	}
	for _, s := range subdirs {
		m.Metrics.Dirs++
		r := s.Name()
		if rel != "" {
			r = rel + "/" + r
		}
		if err := walk(m, filepath.Join(abs, s.Name()), r, depth+1, opt, ignore, files); err != nil && depth == 0 {
			// unreadable subfolder: keep going, remember the first problem
			if m.Error == "" {
				m.Error = fmt.Sprintf("%s: %v", r, err)
			}
		}
	}
	return nil
}

// Signature is a cheap fingerprint: names, sizes and mtimes of entries up to depth 2,
// plus scan options that change the result.
func Signature(dir string, opt Options) uint64 {
	h := fnv.New64a()
	fmt.Fprintf(h, "v%d|dds=%v|arc=%v|plg=%v|rec=%v|", CacheVersion, opt.ReadDDS, opt.Archives, opt.Plugins, opt.RecordScan)
	var rec func(p string, depth int)
	rec = func(p string, depth int) {
		ents, err := os.ReadDir(p)
		if err != nil {
			return
		}
		for _, e := range ents {
			info, err := e.Info()
			if err != nil {
				continue
			}
			fmt.Fprintf(h, "%d|%s|%d|%d;", depth, e.Name(), info.Size(), info.ModTime().UnixNano())
			if e.IsDir() && depth < 2 {
				rec(filepath.Join(p, e.Name()), depth+1)
			}
		}
	}
	rec(dir, 0)
	return h.Sum64()
}

func cachePath(dir, name string) string {
	h := fnv.New64a()
	h.Write([]byte(strings.ToLower(name)))
	return filepath.Join(dir, fmt.Sprintf("%016x.scan", h.Sum64()))
}

func loadCache(dir, name string) (*Mod, error) {
	f, err := os.Open(cachePath(dir, name))
	if err != nil {
		return nil, err
	}
	defer f.Close()
	var m Mod
	if err := gob.NewDecoder(f).Decode(&m); err != nil {
		return nil, err
	}
	if m.Name != name {
		return nil, errors.New("collision")
	}
	return &m, nil
}

func saveCache(dir string, m *Mod) error {
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return err
	}
	p := cachePath(dir, m.Name)
	tmp := p + ".tmp"
	f, err := os.Create(tmp)
	if err != nil {
		return err
	}
	if err := gob.NewEncoder(f).Encode(m); err != nil {
		f.Close()
		os.Remove(tmp)
		return err
	}
	if err := f.Close(); err != nil {
		return err
	}
	return os.Rename(tmp, p)
}
