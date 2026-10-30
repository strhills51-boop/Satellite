// Package engine owns the single source of truth: instance, modlist order, scan table,
// conflict projection and the user layer. Everything the UI shows is derived from it.
package engine

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"runtime/debug"
	"sort"
	"strings"
	"sync"
	"time"

	"modlisttool/internal/formats"
	"modlisttool/internal/index"
	"modlisttool/internal/mo2"
	"modlisttool/internal/scan"
)

// OverwriteName is the pseudo-mod for MO2's overwrite folder.
const OverwriteName = "Overwrite"

// Event is pushed to UI subscribers.
type Event struct {
	Type string `json:"type"`
	Data any    `json:"data,omitempty"`
}

// StageState is one analysis stage in the run modal.
type StageState struct {
	ID      string  `json:"id"`
	Title   string  `json:"title"`
	Status  string  `json:"status"` // pending|active|done|error|skipped
	Text    string  `json:"text"`
	Percent float64 `json:"percent"`
}

// RunState describes the current or last analysis run.
type RunState struct {
	Active    bool         `json:"active"`
	StartedAt time.Time    `json:"startedAt"`
	EndedAt   time.Time    `json:"endedAt"`
	Stages    []StageState `json:"stages"`
	Log       []string     `json:"log"`
	Error     string       `json:"error,omitempty"`
}

// ScanOptions are user-facing analysis toggles.
type ScanOptions struct {
	Force      bool     `json:"force"`
	DDS        bool     `json:"dds"`
	Archives   bool     `json:"archives"`
	Plugins    bool     `json:"plugins"`
	Records    bool     `json:"records"`
	Workers    int      `json:"workers"`
	Ignore     []string `json:"ignore"`
	OnlyActive bool     `json:"onlyActive"`
}

// DefaultScanOptions mirrors the monolith's defaults.
func DefaultScanOptions() ScanOptions {
	return ScanOptions{DDS: true, Archives: true, Plugins: true, Records: false, Ignore: []string{"meta.ini", "desktop.ini", "thumbs.db"}}
}

// Engine is safe for concurrent use.
type Engine struct {
	mu sync.RWMutex

	Inst    *mo2.Instance
	Entries []mo2.Entry // display order, lowest priority first
	dirty   bool        // order changed vs modlist.txt on disk

	plugins   []mo2.PluginLine
	loadorder []string

	scans    []*scan.Mod
	modIndex map[string]uint32 // lowercased mod name -> table mod id
	table    *index.Table
	conf     *index.Conflicts
	confGen  int
	scanOpts ScanOptions
	lastScan time.Time

	metas map[string]mo2.ModMeta // lowercased name -> meta.ini (mods and separators)

	User *UserLayer
	run  RunState

	undo []undoRec
	redo []undoRec

	subMu sync.Mutex
	subs  map[chan Event]struct{}
}

type undoRec struct {
	Label   string
	Entries []mo2.Entry
}

// New creates an empty engine.
func New() *Engine {
	return &Engine{subs: map[chan Event]struct{}{}, metas: map[string]mo2.ModMeta{}, scanOpts: DefaultScanOptions()}
}

// Subscribe returns an event channel; call the returned func to unsubscribe.
func (e *Engine) Subscribe() (chan Event, func()) {
	ch := make(chan Event, 256)
	e.subMu.Lock()
	e.subs[ch] = struct{}{}
	e.subMu.Unlock()
	return ch, func() {
		e.subMu.Lock()
		delete(e.subs, ch)
		e.subMu.Unlock()
	}
}

func (e *Engine) emit(t string, d any) {
	e.subMu.Lock()
	defer e.subMu.Unlock()
	for ch := range e.subs {
		select {
		case ch <- Event{Type: t, Data: d}:
		default: // slow consumer: drop
		}
	}
}

// WorkspaceDir is where project data lives: <profile>/monolith.
func (e *Engine) WorkspaceDir() string {
	if e.Inst == nil {
		return ""
	}
	return filepath.Join(e.Inst.ProfileDir(), "monolith")
}

func (e *Engine) cacheDir() string { return filepath.Join(e.WorkspaceDir(), "cache") }

// Open loads an instance and its active profile.
func (e *Engine) Open(path string, extra []string, profile string) error {
	in, err := mo2.LoadInstance(path)
	if err != nil {
		return err
	}
	in.ExtraModDirs = cleanDirs(extra)
	if profile != "" {
		in.Profile = profile
	}
	data, err := os.ReadFile(filepath.Join(in.ProfileDir(), "modlist.txt"))
	if err != nil {
		return fmt.Errorf("modlist.txt профиля %q: %w", in.Profile, err)
	}
	e.mu.Lock()
	e.Inst = in
	e.Entries = mo2.ParseModlist(data)
	e.dirty = false
	e.undo, e.redo = nil, nil
	e.scans, e.table, e.conf, e.modIndex = nil, nil, nil, nil
	pdata, _ := os.ReadFile(filepath.Join(in.ProfileDir(), "plugins.txt"))
	ldata, _ := os.ReadFile(filepath.Join(in.ProfileDir(), "loadorder.txt"))
	e.plugins = mo2.ParsePlugins(pdata)
	e.loadorder = mo2.ParseLoadorder(ldata)
	e.User = LoadUserLayer(filepath.Join(e.WorkspaceDir(), "project.json"))
	if pending := e.User.PendingOrder; len(pending) > 0 && e.User.PendingBase == hashEntries(e.Entries) {
		e.Entries = pending
		e.dirty = true
	}
	e.metas = map[string]mo2.ModMeta{}
	e.mu.Unlock()
	e.loadMetas()
	e.journal("open", fmt.Sprintf("Открыт профиль %s (%d записей)", in.Profile, len(e.Entries)))
	e.emit("opened", nil)
	// Restore the last analysis from the scan cache (unchanged mods are not re-walked).
	if !e.User.LastScan.IsZero() {
		opt := DefaultScanOptions()
		if e.User.ScanOptions != nil {
			opt = *e.User.ScanOptions
		}
		opt.Force = false
		_ = e.StartScan(opt)
	}
	return nil
}

func cleanDirs(ds []string) []string {
	var out []string
	for _, d := range ds {
		d = strings.TrimSpace(strings.Trim(d, `"`))
		if d != "" {
			out = append(out, filepath.Clean(d))
		}
	}
	return out
}

// ResolveModDir finds the folder of a mod across all mod roots.
func (e *Engine) ResolveModDir(name string) string {
	if name == OverwriteName {
		return e.Inst.OverwriteDir
	}
	roots := e.Inst.ModRoots()
	for _, r := range roots {
		p := filepath.Join(r, name)
		if st, err := os.Stat(p); err == nil && st.IsDir() {
			return p
		}
	}
	return filepath.Join(roots[0], name)
}

func (e *Engine) loadMetas() {
	e.mu.RLock()
	names := make([]string, 0, len(e.Entries))
	for _, en := range e.Entries {
		if !en.Unmanaged {
			names = append(names, en.Name)
		}
	}
	e.mu.RUnlock()
	metas := make(map[string]mo2.ModMeta, len(names))
	var mu sync.Mutex
	var wg sync.WaitGroup
	ch := make(chan string)
	for w := 0; w < 16; w++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for n := range ch {
				m := mo2.ReadModMeta(e.ResolveModDir(n))
				mu.Lock()
				metas[strings.ToLower(n)] = m
				mu.Unlock()
			}
		}()
	}
	for _, n := range names {
		ch <- n
	}
	close(ch)
	wg.Wait()
	e.mu.Lock()
	e.metas = metas
	e.mu.Unlock()
}

// ---------------------------------------------------------------- analysis

var stageDefs = []StageState{
	{ID: "surface", Title: "🫧 Поверхность — сигнатуры модов"},
	{ID: "deep", Title: "🌌 Бездна — полный обход файлов, архивы, плагины, DDS"},
	{ID: "index", Title: "🗜️ Индекс путей (VFS)"},
	{ID: "overwrite", Title: "⚔️ Владельцы файлов → цепочки → контексты"},
	{ID: "rules", Title: "🏷️ Теги и стикеры по правилам"},
	{ID: "finish", Title: "✅ Сохранение и подготовка интерфейса"},
}

// RunState returns a copy of the current run state.
func (e *Engine) RunState() RunState {
	e.mu.RLock()
	defer e.mu.RUnlock()
	r := e.run
	r.Stages = append([]StageState(nil), e.run.Stages...)
	r.Log = append([]string(nil), e.run.Log...)
	return r
}

func (e *Engine) stage(id, status, text string, pct float64) {
	e.mu.Lock()
	for i := range e.run.Stages {
		if e.run.Stages[i].ID == id {
			s := &e.run.Stages[i]
			s.Status, s.Text, s.Percent = status, text, pct
			cp := *s
			e.mu.Unlock()
			e.emit("stage", cp)
			return
		}
	}
	e.mu.Unlock()
}

func (e *Engine) logf(format string, a ...any) {
	line := time.Now().Format("15:04:05") + "  " + fmt.Sprintf(format, a...)
	e.mu.Lock()
	e.run.Log = append(e.run.Log, line)
	if len(e.run.Log) > 2000 {
		e.run.Log = e.run.Log[len(e.run.Log)-2000:]
	}
	e.mu.Unlock()
	e.emit("log", line)
}

// StartScan runs the analysis pipeline in the background.
func (e *Engine) StartScan(opt ScanOptions) error {
	e.mu.Lock()
	if e.Inst == nil {
		e.mu.Unlock()
		return errors.New("профиль не открыт")
	}
	if e.run.Active {
		e.mu.Unlock()
		return errors.New("пробег уже идёт")
	}
	if len(opt.Ignore) == 0 {
		opt.Ignore = DefaultScanOptions().Ignore
	}
	e.run = RunState{Active: true, StartedAt: time.Now(), Stages: append([]StageState(nil), stageDefs...)}
	for i := range e.run.Stages {
		e.run.Stages[i].Status = "pending"
	}
	e.mu.Unlock()
	e.emit("run", e.RunState())
	go func() {
		err := e.runPipeline(opt)
		e.mu.Lock()
		e.run.Active = false
		e.run.EndedAt = time.Now()
		if err != nil {
			e.run.Error = err.Error()
		}
		e.mu.Unlock()
		if err != nil {
			e.logf("Ошибка: %v", err)
		}
		e.emit("run", e.RunState())
		e.emit("changed", "scan")
	}()
	return nil
}

func (e *Engine) runPipeline(opt ScanOptions) error {
	t0 := time.Now()
	e.mu.RLock()
	var jobs []scan.Job
	for _, en := range e.Entries {
		if en.Separator || en.Unmanaged {
			continue
		}
		if opt.OnlyActive && !en.Enabled {
			continue
		}
		jobs = append(jobs, scan.Job{Name: en.Name})
	}
	e.mu.RUnlock()
	for i := range jobs {
		jobs[i].Path = e.ResolveModDir(jobs[i].Name)
	}
	jobs = append(jobs, scan.Job{Name: OverwriteName, Path: e.Inst.OverwriteDir})
	e.logf("Пробег: %d модов, корни: %s", len(jobs), strings.Join(e.Inst.ModRoots(), " ; "))
	e.stage("surface", "active", "Подсчёт сигнатур…", 0)
	sopt := scan.Options{Workers: opt.Workers, CacheDir: e.cacheDir(), Force: opt.Force, ReadDDS: opt.DDS, Archives: opt.Archives, Plugins: opt.Plugins, RecordScan: opt.Records, IgnoreNames: opt.Ignore}
	e.stage("surface", "done", "Сигнатуры проверяются внутри обхода (кеш по сигнатуре)", 100)
	e.stage("deep", "active", "Обход…", 0)
	var lastEmit time.Time
	mods := scan.Run(jobs, sopt, func(p scan.Progress) {
		if time.Since(lastEmit) < 120*time.Millisecond && p.Done < p.Total {
			return
		}
		lastEmit = time.Now()
		pct := 100 * float64(p.Done) / float64(max(1, p.Total))
		e.stage("deep", "active", fmt.Sprintf("%d / %d модов · из кеша %d · файлов %d · %s", p.Done, p.Total, p.Cached, p.Files, p.Current), pct)
	})
	cached, missing, errs, files, arch := 0, 0, 0, 0, 0
	for _, m := range mods {
		if m.FromCache {
			cached++
		}
		if m.Missing {
			missing++
			if m.Name != OverwriteName {
				e.logf("Нет папки: %s", m.Name)
			}
		}
		if m.Error != "" {
			errs++
			e.logf("Ошибка чтения %s: %s", m.Name, m.Error)
		}
		files += m.Metrics.Files
		arch += m.Metrics.ArchiveFile
	}
	e.stage("deep", "done", fmt.Sprintf("Модов %d · из кеша %d · нет папки %d · ошибок %d · файлов %d · в архивах %d · %.1fs", len(mods), cached, missing, errs, files, arch, time.Since(t0).Seconds()), 100)
	e.logf("Обход завершён за %.1fs", time.Since(t0).Seconds())

	t1 := time.Now()
	e.stage("index", "active", "Интернирование путей и сортировка…", 10)
	tb := index.Build(mods)
	// The table now owns all paths; drop per-file listings from scans (they stay in the disk cache).
	for _, m := range mods {
		m.Dirs = nil
		for i := range m.Archives {
			m.Archives[i].Entries = nil
		}
	}
	runtime.GC()
	debug.FreeOSMemory()
	e.stage("index", "done", fmt.Sprintf("Путей %d · провайдеров %d · каталогов %d · имён %d · %.1fs", tb.NumPaths(), len(tb.Key), len(tb.In.DirParent), len(tb.In.Names), time.Since(t1).Seconds()), 100)

	e.mu.Lock()
	e.scans = mods
	e.table = tb
	e.modIndex = make(map[string]uint32, len(tb.Mods))
	for i, n := range tb.Mods {
		e.modIndex[strings.ToLower(n)] = uint32(i)
	}
	e.scanOpts = opt
	e.lastScan = time.Now()
	e.mu.Unlock()

	t2 := time.Now()
	e.stage("overwrite", "active", "Стеки владельцев…", 30)
	e.recompute()
	c := e.Conflicts()
	e.stage("overwrite", "done", fmt.Sprintf("Конфликтных путей %d · цепочек %d · контекстов %d · %.2fs", c.ConflictPaths, len(c.Chains), len(c.Contexts), time.Since(t2).Seconds()), 100)

	e.stage("rules", "active", "Применение правил…", 0)
	n := e.applyRules()
	e.stage("rules", "done", fmt.Sprintf("Назначено меток: %d", n), 100)

	e.stage("finish", "active", "Сохранение…", 50)
	e.mu.Lock()
	e.User.LastScan = time.Now()
	e.User.ScanOptions = &opt
	e.mu.Unlock()
	e.saveUser()
	e.stage("finish", "done", fmt.Sprintf("Готово за %.1fs", time.Since(t0).Seconds()), 100)
	e.logf("Пробег завершён: %.1fs", time.Since(t0).Seconds())
	e.journal("scan", fmt.Sprintf("Пробег: %d модов, %d путей, %d конфликтов", len(mods), tb.NumPaths(), c.ConflictPaths))
	return nil
}

// priorities derives per-table-mod priority from display order (higher wins).
func (e *Engine) priorities() []int32 {
	prio := make([]int32, len(e.table.Mods))
	for i := range prio {
		prio[i] = -1
	}
	p := int32(0)
	for _, en := range e.Entries {
		if en.Separator || en.Unmanaged {
			continue
		}
		p++
		if !en.Enabled {
			continue
		}
		if id, ok := e.modIndex[strings.ToLower(en.Name)]; ok {
			prio[id] = p
		}
	}
	if id, ok := e.modIndex[strings.ToLower(OverwriteName)]; ok {
		prio[id] = p + 1
	}
	return prio
}

// recompute rebuilds the order-dependent projection (fast: no disk access).
func (e *Engine) recompute() {
	e.mu.RLock()
	if e.table == nil {
		e.mu.RUnlock()
		return
	}
	tb := e.table
	prio := e.priorities()
	e.mu.RUnlock()
	c := index.Compute(tb, prio)
	e.mu.Lock()
	if e.table == tb {
		e.conf = c
		e.confGen++
	}
	e.mu.Unlock()
}

// Conflicts returns the current projection (may be nil).
func (e *Engine) Conflicts() *index.Conflicts {
	e.mu.RLock()
	defer e.mu.RUnlock()
	return e.conf
}

// ---------------------------------------------------------------- order mutations

func hashEntries(es []mo2.Entry) string {
	b := mo2.SerializeModlist(es)
	return fmt.Sprintf("%x-%d", fnv64(b), len(b))
}

func fnv64(b []byte) uint64 {
	h := uint64(1469598103934665603)
	for _, c := range b {
		h ^= uint64(c)
		h *= 1099511628211
	}
	return h
}

func (e *Engine) pushUndo(label string) {
	e.undo = append(e.undo, undoRec{Label: label, Entries: append([]mo2.Entry(nil), e.Entries...)})
	if len(e.undo) > 200 {
		e.undo = e.undo[1:]
	}
	e.redo = nil
}

func (e *Engine) afterOrderChange(label string) {
	data, _ := os.ReadFile(filepath.Join(e.Inst.ProfileDir(), "modlist.txt"))
	base := hashEntries(mo2.ParseModlist(data))
	e.dirty = base != hashEntries(e.Entries)
	if e.dirty {
		e.User.PendingBase = base
		e.User.PendingOrder = append([]mo2.Entry(nil), e.Entries...)
	} else {
		e.User.PendingBase, e.User.PendingOrder = "", nil
	}
	go func() {
		e.recompute()
		e.saveUser()
		e.emit("changed", label)
	}()
}

func (e *Engine) indexOf(name string) int {
	for i, en := range e.Entries {
		if en.Name == name {
			return i
		}
	}
	return -1
}

// Move moves entries (by name) so they land before the entry `before` ("" = end).
// Separators move together with their block of mods.
func (e *Engine) Move(names []string, before string) error {
	e.mu.Lock()
	defer e.mu.Unlock()
	if len(names) == 0 {
		return nil
	}
	pick := map[int]bool{}
	for _, n := range names {
		i := e.indexOf(n)
		if i < 0 {
			return fmt.Errorf("нет записи %q", n)
		}
		pick[i] = true
		if e.Entries[i].Separator {
			for j := i + 1; j < len(e.Entries) && !e.Entries[j].Separator; j++ {
				pick[j] = true
			}
		}
	}
	if before != "" {
		if bi := e.indexOf(before); bi >= 0 && pick[bi] {
			return nil // dropping onto itself
		}
	}
	e.pushUndo("move")
	var moved, rest []mo2.Entry
	for i, en := range e.Entries {
		if pick[i] {
			moved = append(moved, en)
		} else {
			rest = append(rest, en)
		}
	}
	at := len(rest)
	if before != "" {
		for i, en := range rest {
			if en.Name == before {
				at = i
				break
			}
		}
	}
	out := make([]mo2.Entry, 0, len(e.Entries))
	out = append(out, rest[:at]...)
	out = append(out, moved...)
	out = append(out, rest[at:]...)
	e.Entries = out
	e.afterOrderChange("move")
	return nil
}

// SetEnabled toggles mods.
func (e *Engine) SetEnabled(names []string, enabled *bool) error {
	e.mu.Lock()
	defer e.mu.Unlock()
	e.pushUndo("toggle")
	for _, n := range names {
		i := e.indexOf(n)
		if i < 0 || e.Entries[i].Separator || e.Entries[i].Unmanaged {
			continue
		}
		if enabled == nil {
			e.Entries[i].Enabled = !e.Entries[i].Enabled
		} else {
			e.Entries[i].Enabled = *enabled
		}
	}
	e.afterOrderChange("toggle")
	return nil
}

// Undo / Redo restore order snapshots.
func (e *Engine) Undo() bool {
	e.mu.Lock()
	defer e.mu.Unlock()
	if len(e.undo) == 0 {
		return false
	}
	r := e.undo[len(e.undo)-1]
	e.undo = e.undo[:len(e.undo)-1]
	e.redo = append(e.redo, undoRec{Label: r.Label, Entries: append([]mo2.Entry(nil), e.Entries...)})
	e.Entries = r.Entries
	e.afterOrderChange("undo")
	return true
}

func (e *Engine) Redo() bool {
	e.mu.Lock()
	defer e.mu.Unlock()
	if len(e.redo) == 0 {
		return false
	}
	r := e.redo[len(e.redo)-1]
	e.redo = e.redo[:len(e.redo)-1]
	e.undo = append(e.undo, undoRec{Label: r.Label, Entries: append([]mo2.Entry(nil), e.Entries...)})
	e.Entries = r.Entries
	e.afterOrderChange("redo")
	return true
}

// SaveModlist writes modlist.txt with a backup version in monolith/versions/<ts>/.
func (e *Engine) SaveModlist() (string, error) {
	e.mu.Lock()
	defer e.mu.Unlock()
	if e.Inst == nil {
		return "", errors.New("профиль не открыт")
	}
	prof := e.Inst.ProfileDir()
	target := filepath.Join(prof, "modlist.txt")
	old, err := os.ReadFile(target)
	if err != nil {
		return "", err
	}
	ver := filepath.Join(e.WorkspaceDir(), "versions", time.Now().Format("2006-01-02_15-04-05"))
	if err := mo2.WriteFileAtomic(filepath.Join(ver, "modlist.txt"), old); err != nil {
		return "", fmt.Errorf("бэкап: %w", err)
	}
	if err := mo2.WriteFileAtomic(target, mo2.SerializeModlist(e.Entries)); err != nil {
		return "", err
	}
	e.dirty = false
	e.User.PendingOrder, e.User.PendingBase = nil, ""
	go e.saveUser()
	go e.journal("save_modlist", "Сохранён modlist.txt (бэкап: "+filepath.Base(ver)+")")
	return ver, nil
}

// ---------------------------------------------------------------- views

// Row is one entry of the modlist as the UI sees it.
type Row struct {
	I         int              `json:"i"`
	Name      string           `json:"name"`
	Display   string           `json:"display"`
	Sep       bool             `json:"sep,omitempty"`
	Enabled   bool             `json:"en"`
	Unmanaged bool             `json:"unm,omitempty"`
	SepIndex  int              `json:"sepI"` // index of owning separator row, -1 none
	Prio      int              `json:"prio,omitempty"`
	Color     string           `json:"color,omitempty"`
	Stat      *index.ModStat   `json:"st,omitempty"`
	Missing   bool             `json:"missing,omitempty"`
	Size      int64            `json:"size,omitempty"`
	Plugins   int              `json:"plg,omitempty"`
	User      *ModUser         `json:"u,omitempty"`
	Version   string           `json:"ver,omitempty"`
}

// ModlistView is the main classic-view payload.
type ModlistView struct {
	Profile   string        `json:"profile"`
	Instance  *mo2.Instance `json:"instance"`
	Rows      []Row         `json:"rows"`
	Dirty     bool          `json:"dirty"`
	Analyzed  bool          `json:"analyzed"`
	LastScan  time.Time     `json:"lastScan"`
	Gen       int           `json:"gen"`
	CanUndo   bool          `json:"canUndo"`
	CanRedo   bool          `json:"canRedo"`
	Overwrite *Row          `json:"overwrite,omitempty"`
}

// Modlist returns the current classic view.
func (e *Engine) Modlist() ModlistView {
	e.mu.RLock()
	defer e.mu.RUnlock()
	v := ModlistView{Instance: e.Inst, Dirty: e.dirty, Analyzed: e.conf != nil, LastScan: e.lastScan, Gen: e.confGen, CanUndo: len(e.undo) > 0, CanRedo: len(e.redo) > 0}
	if e.Inst == nil {
		return v
	}
	v.Profile = e.Inst.Profile
	v.Rows = make([]Row, len(e.Entries))
	sepI, sepColor := -1, ""
	prio := 0
	for i, en := range e.Entries {
		r := Row{I: i, Name: en.Name, Display: en.DisplayName(), Sep: en.Separator, Enabled: en.Enabled, Unmanaged: en.Unmanaged}
		meta := e.metas[strings.ToLower(en.Name)]
		if en.Separator {
			sepI = i
			sepColor = meta.Color
			if u := e.User.Seps[en.Name]; u != nil && u.Color != "" {
				sepColor = u.Color
			}
			r.Color = sepColor
			r.SepIndex = -1
		} else {
			r.SepIndex = sepI
			r.Color = sepColor
			r.Version = meta.Version
			if !en.Unmanaged {
				prio++
				r.Prio = prio
			}
		}
		if u := e.User.Mods[en.Name]; u != nil {
			r.User = u
			if u.Color != "" {
				r.Color = u.Color
			}
		}
		e.fillStats(&r, en.Name)
		v.Rows[i] = r
	}
	ow := Row{I: -1, Name: OverwriteName, Display: OverwriteName, Enabled: true, SepIndex: -1, Prio: prio + 1}
	e.fillStats(&ow, OverwriteName)
	v.Overwrite = &ow
	return v
}

func (e *Engine) fillStats(r *Row, name string) {
	if e.table == nil {
		return
	}
	id, ok := e.modIndex[strings.ToLower(name)]
	if !ok {
		return
	}
	if m := e.scans[id]; m != nil {
		r.Missing = m.Missing
		r.Size = m.Metrics.Bytes
		r.Plugins = m.Metrics.Plugins
	}
	if e.conf != nil {
		st := e.conf.Stats[id]
		r.Stat = &st
	}
}

// ModDetails is the right-panel summary of one mod.
type ModDetails struct {
	Name     string            `json:"name"`
	Path     string            `json:"path"`
	Meta     mo2.ModMeta       `json:"meta"`
	Metrics  *scan.Metrics     `json:"metrics,omitempty"`
	Plugins  []*formats.Plugin `json:"plugins,omitempty"`
	Archives []index.ArchiveRef `json:"archives,omitempty"`
	Stat     *index.ModStat    `json:"stat,omitempty"`
	User     *ModUser          `json:"user,omitempty"`
	Scanned  time.Time         `json:"scanned"`
	Cached   bool              `json:"cached"`
	Missing  bool              `json:"missing"`
	Error    string            `json:"error,omitempty"`
}

// Mod returns details for one mod.
func (e *Engine) Mod(name string) (*ModDetails, error) {
	e.mu.RLock()
	defer e.mu.RUnlock()
	if e.Inst == nil {
		return nil, errors.New("профиль не открыт")
	}
	d := &ModDetails{Name: name, Path: e.ResolveModDir(name), Meta: e.metas[strings.ToLower(name)], User: e.User.Mods[name]}
	if e.table == nil {
		return d, nil
	}
	id, ok := e.modIndex[strings.ToLower(name)]
	if !ok {
		return d, nil
	}
	if m := e.scans[id]; m != nil {
		mt := m.Metrics
		d.Metrics, d.Plugins, d.Scanned, d.Cached, d.Missing, d.Error = &mt, m.Plugins, m.ScannedAt, m.FromCache, m.Missing, m.Error
	}
	for _, a := range e.table.Archives {
		if a.Mod == id {
			d.Archives = append(d.Archives, a)
		}
	}
	if e.conf != nil {
		st := e.conf.Stats[id]
		d.Stat = &st
	}
	return d, nil
}

// ChainView is a chain with mod names resolved.
type ChainView struct {
	ID      int32    `json:"id"`
	Key     string   `json:"key"`
	Mods    []string `json:"mods"` // winner first
	Files   uint32   `json:"files"`
	Bytes   uint64   `json:"bytes"`
	Context int32    `json:"context"`
	Done    bool     `json:"done"`
	Label   string   `json:"label,omitempty"`
	Color   string   `json:"color,omitempty"`
	Icon    string   `json:"icon,omitempty"`
	Role    string   `json:"role,omitempty"` // W/L/M for the requested mod
}

func (e *Engine) chainView(ch *index.Chain) ChainView {
	v := ChainView{ID: ch.ID, Files: ch.Files, Bytes: ch.Bytes, Context: ch.Context}
	keys := make([]string, len(ch.Mods))
	for i, m := range ch.Mods {
		v.Mods = append(v.Mods, e.table.Mods[m])
		keys[i] = strings.ToLower(e.table.Mods[m])
	}
	// stable key: sorted set of member names (survives reorder)
	sort.Strings(keys)
	v.Key = strings.Join(keys, "\x1f")
	if u := e.User.Chains[v.Key]; u != nil {
		v.Done, v.Label, v.Color, v.Icon = u.Done, u.Label, u.Color, u.Icon
	}
	return v
}

// ModChains returns chains a mod participates in.
func (e *Engine) ModChains(name string) ([]ChainView, error) {
	e.mu.RLock()
	defer e.mu.RUnlock()
	if e.conf == nil {
		return nil, errors.New("анализ не выполнен")
	}
	id, ok := e.modIndex[strings.ToLower(name)]
	if !ok {
		return nil, nil
	}
	var out []ChainView
	for _, cid := range e.conf.ModChainIDs(id) {
		ch := &e.conf.Chains[cid]
		v := e.chainView(ch)
		switch {
		case ch.Mods[0] == id:
			v.Role = "W"
		case ch.Mods[len(ch.Mods)-1] == id:
			v.Role = "L"
		default:
			v.Role = "M"
		}
		out = append(out, v)
	}
	sort.Slice(out, func(a, b int) bool { return out[a].Files > out[b].Files })
	return out, nil
}

// Provider is one owner of a path.
type Provider struct {
	Mod     string `json:"mod"`
	Archive string `json:"archive,omitempty"`
	Size    uint64 `json:"size"`
	VRAMKiB uint32 `json:"vramKiB,omitempty"`
	Active  bool   `json:"active"`
	Winner  bool   `json:"winner"`
}

// PathView is one virtual path with its stack.
type PathView struct {
	Path  string     `json:"path"`
	Stack []Provider `json:"stack"`
}

// Page is a paged result.
type Page[T any] struct {
	Total  int `json:"total"`
	Offset int `json:"offset"`
	Items  []T `json:"items"`
}

func (e *Engine) pathView(p uint32) PathView {
	t := e.table
	pv := PathView{Path: t.PathString(p)}
	win := e.conf.PathWinner[p]
	type it struct {
		pr  Provider
		key int
	}
	var items []it
	for i := t.PathOff[p]; i < t.PathOff[p+1]; i++ {
		m := t.Mod[i]
		pr := Provider{Mod: t.Mods[m], Size: t.Size[i], VRAMKiB: t.VRAM[i], Active: e.conf.Prio[m] >= 0}
		if a := t.Arch[i]; a > 0 {
			pr.Archive = t.Archives[a-1].Name
		}
		pr.Winner = int32(m) == win && (pr.Archive == "" || !hasLoose(t, p, m))
		k := int(e.conf.Prio[m]) * 2
		if pr.Archive == "" {
			k += 1 << 30
		}
		items = append(items, it{pr, k})
	}
	sort.SliceStable(items, func(a, b int) bool { return items[a].key > items[b].key })
	for _, x := range items {
		pv.Stack = append(pv.Stack, x.pr)
	}
	return pv
}

func hasLoose(t *index.Table, p uint32, m uint32) bool {
	for i := t.PathOff[p]; i < t.PathOff[p+1]; i++ {
		if t.Mod[i] == m && t.Arch[i] == 0 {
			return true
		}
	}
	return false
}

// ChainFiles returns a page of files of a chain, optionally filtered by substring.
func (e *Engine) ChainFiles(id int32, q string, offset, limit int) (*Page[PathView], error) {
	e.mu.RLock()
	defer e.mu.RUnlock()
	if e.conf == nil || id < 0 || int(id) >= len(e.conf.Chains) {
		return nil, errors.New("нет цепочки")
	}
	ids := e.conf.ChainPathIDs(id)
	return e.pagePaths(ids, q, offset, limit), nil
}

func (e *Engine) pagePaths(ids []uint32, q string, offset, limit int) *Page[PathView] {
	q = strings.ToLower(q)
	if limit <= 0 || limit > 2000 {
		limit = 200
	}
	pg := &Page[PathView]{Offset: offset}
	for _, p := range ids {
		if q != "" && !strings.Contains(strings.ToLower(e.table.PathString(p)), q) {
			continue
		}
		if pg.Total >= offset && len(pg.Items) < limit {
			pg.Items = append(pg.Items, e.pathView(p))
		}
		pg.Total++
	}
	return pg
}

// ContextView summarizes a context.
type ContextView struct {
	ID     int32       `json:"id"`
	Mods   []string    `json:"mods"`
	Files  uint32      `json:"files"`
	Chains []ChainView `json:"chains"`
	Done   int         `json:"done"`
}

// Context returns one context (chains sorted by size).
func (e *Engine) Context(id int32) (*ContextView, error) {
	e.mu.RLock()
	defer e.mu.RUnlock()
	if e.conf == nil || id < 0 || int(id) >= len(e.conf.Contexts) {
		return nil, errors.New("нет контекста")
	}
	c := &e.conf.Contexts[id]
	v := &ContextView{ID: id, Files: c.Files}
	for _, m := range c.Mods {
		v.Mods = append(v.Mods, e.table.Mods[m])
	}
	for _, cid := range c.Chains {
		cv := e.chainView(&e.conf.Chains[cid])
		if cv.Done {
			v.Done++
		}
		v.Chains = append(v.Chains, cv)
	}
	sort.Slice(v.Chains, func(a, b int) bool { return v.Chains[a].Files > v.Chains[b].Files })
	if len(v.Chains) > 5000 {
		v.Chains = v.Chains[:5000]
	}
	return v, nil
}

// Contexts lists all contexts briefly.
func (e *Engine) Contexts() []ContextView {
	e.mu.RLock()
	defer e.mu.RUnlock()
	if e.conf == nil {
		return nil
	}
	out := make([]ContextView, 0, len(e.conf.Contexts))
	for _, c := range e.conf.Contexts {
		v := ContextView{ID: c.ID, Files: c.Files}
		for i, m := range c.Mods {
			if i >= 12 {
				break
			}
			v.Mods = append(v.Mods, e.table.Mods[m])
		}
		out = append(out, v)
	}
	sort.Slice(out, func(a, b int) bool { return out[a].Files > out[b].Files })
	return out
}

// TreeEntry is one row in a mod's directory listing.
type TreeEntry struct {
	Name    string `json:"name"`
	Dir     bool   `json:"dir"`
	Files   int    `json:"files,omitempty"` // dirs: files below
	Size    uint64 `json:"size"`
	Archive string `json:"archive,omitempty"`
	State   string `json:"state,omitempty"` // W win / L lose / U unique / - inactive
	Others  int    `json:"others,omitempty"`
	Path    string `json:"path,omitempty"`
}

// Tree lists one directory of a mod (dir "" = root). Built on demand from the mod's providers.
func (e *Engine) Tree(name, dir string) ([]TreeEntry, error) {
	e.mu.RLock()
	defer e.mu.RUnlock()
	if e.table == nil {
		return nil, errors.New("анализ не выполнен")
	}
	id, ok := e.modIndex[strings.ToLower(name)]
	if !ok {
		return nil, errors.New("мод не просканирован")
	}
	t := e.table
	dir = strings.ToLower(strings.Trim(dir, "/"))
	dirs := map[string]*TreeEntry{}
	var files []TreeEntry
	for _, pi := range t.ModProv[t.ModOff[id]:t.ModOff[id+1]] {
		full := t.In.KeyPath(t.Key[pi])
		lf := strings.ToLower(full)
		rest := lf
		if dir != "" {
			if !strings.HasPrefix(lf, dir+"/") {
				continue
			}
			rest = lf[len(dir)+1:]
		}
		disp := full[len(full)-len(rest):]
		if i := strings.IndexByte(rest, '/'); i >= 0 {
			dn := disp[:i]
			d := dirs[strings.ToLower(dn)]
			if d == nil {
				d = &TreeEntry{Name: dn, Dir: true}
				dirs[strings.ToLower(dn)] = d
			}
			d.Files++
			d.Size += t.Size[pi]
			continue
		}
		te := TreeEntry{Name: disp, Size: t.Size[pi], Path: full}
		if a := t.Arch[pi]; a > 0 {
			te.Archive = t.Archives[a-1].Name
		}
		if e.conf != nil {
			p := t.PathOfProvider(pi)
			others := 0
			for i := t.PathOff[p]; i < t.PathOff[p+1]; i++ {
				if t.Mod[i] != id && e.conf.Prio[t.Mod[i]] >= 0 {
					others++
				}
			}
			te.Others = others
			switch {
			case e.conf.Prio[id] < 0:
				te.State = "-"
			case others == 0:
				te.State = "U"
			case e.conf.PathWinner[p] == int32(id) && (te.Archive == "" || !hasLoose(t, p, id)):
				te.State = "W"
			default:
				te.State = "L"
			}
		}
		files = append(files, te)
	}
	out := make([]TreeEntry, 0, len(dirs)+len(files))
	for _, d := range dirs {
		out = append(out, *d)
	}
	sort.Slice(out, func(a, b int) bool { return strings.ToLower(out[a].Name) < strings.ToLower(out[b].Name) })
	sort.Slice(files, func(a, b int) bool { return strings.ToLower(files[a].Name) < strings.ToLower(files[b].Name) })
	return append(out, files...), nil
}

// SearchFiles finds virtual paths containing q.
func (e *Engine) SearchFiles(q string, offset, limit int) (*Page[PathView], error) {
	e.mu.RLock()
	defer e.mu.RUnlock()
	if e.conf == nil {
		return nil, errors.New("анализ не выполнен")
	}
	q = strings.ToLower(strings.TrimSpace(q))
	if len(q) < 2 {
		return &Page[PathView]{}, nil
	}
	if limit <= 0 || limit > 1000 {
		limit = 200
	}
	pg := &Page[PathView]{Offset: offset}
	t := e.table
	for p := 0; p < t.NumPaths(); p++ {
		if !strings.Contains(strings.ToLower(t.PathString(uint32(p))), q) {
			continue
		}
		if pg.Total >= offset && len(pg.Items) < limit {
			pg.Items = append(pg.Items, e.pathView(uint32(p)))
		}
		pg.Total++
		if pg.Total > 100000 {
			break
		}
	}
	return pg, nil
}

// Summary is the global build summary.
type Summary struct {
	Mods, Enabled, Disabled, Separators int
	Bytes                               int64
	Files, Paths, ConflictPaths         int
	Chains, Contexts                    int
	VRAMKnownBytes                      int64
	VRAMWinBytes                        uint64
	Plugins, PluginsActive              int
	ESM, ESP, ESL, FullSlots            int
	DDS, NIF, PEX, HKX, DLL, Archives   int
	FullyOverridden                     int
	LastScan                            time.Time
	Workspace                           string
}

// Summary computes global counters.
func (e *Engine) Summary() Summary {
	e.mu.RLock()
	defer e.mu.RUnlock()
	var s Summary
	s.Workspace = e.WorkspaceDir()
	s.LastScan = e.lastScan
	for _, en := range e.Entries {
		switch {
		case en.Separator:
			s.Separators++
		case en.Unmanaged:
		default:
			s.Mods++
			if en.Enabled {
				s.Enabled++
			} else {
				s.Disabled++
			}
		}
	}
	for _, m := range e.scans {
		if m == nil {
			continue
		}
		s.Bytes += m.Metrics.Bytes
		s.Files += m.Metrics.Files
		s.VRAMKnownBytes += m.Metrics.VRAMBytes
		s.DDS += m.Metrics.DDS
		s.NIF += m.Metrics.NIF
		s.PEX += m.Metrics.PEX
		s.HKX += m.Metrics.HKX
		s.DLL += m.Metrics.DLL
		s.Archives += m.Metrics.Archives
	}
	if e.table != nil {
		s.Paths = e.table.NumPaths()
	}
	if e.conf != nil {
		s.ConflictPaths = e.conf.ConflictPaths
		s.Chains = len(e.conf.Chains)
		s.Contexts = len(e.conf.Contexts)
		s.VRAMWinBytes = e.conf.VRAMWinKB * 1024
		for _, st := range e.conf.Stats {
			if st.Full {
				s.FullyOverridden++
			}
		}
	}
	pl := e.pluginsLocked()
	s.Plugins = len(pl)
	for _, p := range pl {
		if !p.Enabled {
			continue
		}
		s.PluginsActive++
		switch p.Kind {
		case "ESM":
			s.ESM++
		case "ESL":
			s.ESL++
		default:
			s.ESP++
		}
		if p.Kind != "ESL" {
			s.FullSlots++
		}
	}
	return s
}
