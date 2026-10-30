package index

import (
	"runtime"
	"slices"
	"sync"
)

// Chain is an exact overwrite chain: a set of paths that share the same ordered stack of mods.
type Chain struct {
	ID      int32   `json:"id"`
	Mods    []uint32 `json:"mods"` // winner first
	Files   uint32  `json:"files"`
	Bytes   uint64  `json:"bytes"` // winner bytes
	Context int32   `json:"context"`
}

// Context is a connected component of mods linked through chains.
type Context struct {
	ID     int32    `json:"id"`
	Mods   []uint32 `json:"mods"`
	Chains []int32  `json:"chains"`
	Files  uint32   `json:"files"`
}

// ModStat is the per-mod overwrite summary (W/L/M/X).
type ModStat struct {
	Files     uint32 `json:"files"`     // providers of this mod (loose + archive entries)
	Unique    uint32 `json:"unique"`    // paths only this active mod provides
	Win       uint32 `json:"win"`       // conflict paths this mod wins
	Lose      uint32 `json:"lose"`      // conflict paths where it is overridden
	Mid       uint32 `json:"mid"`       // overridden but itself overrides someone below
	Full      bool   `json:"full"`      // X: nothing of it is visible
	Chains    uint32 `json:"chains"`
	Context   int32  `json:"context"`
	VRAMWinKB uint64 `json:"vramWinKiB"` // VRAM of DDS where this mod is the winner
	BytesWin  uint64 `json:"bytesWin"`
}

// Conflicts is the order-dependent projection over a Table.
type Conflicts struct {
	T        *Table
	Prio     []int32 // per mod; < 0 means inactive
	Chains   []Chain
	Contexts []Context
	Stats    []ModStat

	PathChain  []int32 // per path; -1 when not a conflict
	PathWinner []int32 // per path; winning mod or -1

	ChainFileOff []uint32 // CSR chain -> path ids
	ChainFiles   []uint32
	ModChainOff  []uint32 // CSR mod -> chain ids
	ModChains    []int32

	ConflictPaths int
	ActivePaths   int
	VRAMWinKB     uint64
}

type stackRec struct {
	path uint32
	hash uint64
	off  uint32
	n    uint32
	win  uint64 // winner bytes
}

type chunkOut struct {
	recs    []stackRec
	pool    []uint32
	unique  []uint32
	vram    []uint64
	bytes   []uint64
	active  int
}

// Compute builds the projection for the given priorities (higher wins).
func Compute(t *Table, prio []int32) *Conflicts {
	nm := len(t.Mods)
	np := t.NumPaths()
	c := &Conflicts{T: t, Prio: prio}
	c.PathChain = make([]int32, np)
	c.PathWinner = make([]int32, np)

	workers := runtime.GOMAXPROCS(0)
	chunk := max(4096, (np+workers*4-1)/(workers*4))
	var outs []*chunkOut
	var mu sync.Mutex
	var wg sync.WaitGroup
	type part struct {
		idx    int
		lo, hi int
	}
	var parts []part
	for lo := 0; lo < np; lo += chunk {
		parts = append(parts, part{len(parts), lo, min(np, lo+chunk)})
	}
	outs = make([]*chunkOut, len(parts))
	jobs := make(chan part)
	for w := 0; w < workers; w++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			type cand struct {
				mod  uint32
				arch bool
				pr   int32
				prov uint32
			}
			var buf []cand
			for pt := range jobs {
				o := &chunkOut{unique: make([]uint32, nm), vram: make([]uint64, nm), bytes: make([]uint64, nm)}
				for p := pt.lo; p < pt.hi; p++ {
					lo, hi := t.PathOff[p], t.PathOff[p+1]
					buf = buf[:0]
					for i := lo; i < hi; i++ {
						m := t.Mod[i]
						if prio[m] < 0 {
							continue
						}
						buf = append(buf, cand{m, t.Arch[i] != 0, prio[m], i})
					}
					if len(buf) == 0 {
						c.PathChain[p], c.PathWinner[p] = -1, -1
						continue
					}
					o.active++
					if len(buf) > 1 {
						slices.SortFunc(buf, func(a, b cand) int {
							if a.arch != b.arch { // loose files beat archives
								if a.arch {
									return 1
								}
								return -1
							}
							return int(b.pr) - int(a.pr)
						})
					}
					w := buf[0]
					c.PathWinner[p] = int32(w.mod)
					o.vram[w.mod] += uint64(t.VRAM[w.prov])
					o.bytes[w.mod] += t.Size[w.prov]
					// distinct mods, best rank first
					start := uint32(len(o.pool))
					var h uint64 = 1469598103934665603
					for _, x := range buf {
						dup := false
						for _, y := range o.pool[start:] {
							if y == x.mod {
								dup = true
								break
							}
						}
						if dup {
							continue
						}
						o.pool = append(o.pool, x.mod)
						h ^= uint64(x.mod) + 0x9e3779b97f4a7c15
						h *= 1099511628211
					}
					n := uint32(len(o.pool)) - start
					if n < 2 {
						o.pool = o.pool[:start]
						o.unique[w.mod]++
						c.PathChain[p] = -1
						continue
					}
					o.recs = append(o.recs, stackRec{path: uint32(p), hash: h, off: start, n: n, win: t.Size[w.prov]})
				}
				mu.Lock()
				outs[pt.idx] = o
				mu.Unlock()
			}
		}()
	}
	for _, pt := range parts {
		jobs <- pt
	}
	close(jobs)
	wg.Wait()

	// reduce: intern stacks into chains (sequential, deterministic in path order)
	c.Stats = make([]ModStat, nm)
	byHash := map[uint64][]int32{}
	for _, o := range outs {
		c.ActivePaths += o.active
		for m := 0; m < nm; m++ {
			c.Stats[m].Unique += o.unique[m]
			c.Stats[m].VRAMWinKB += o.vram[m]
			c.Stats[m].BytesWin += o.bytes[m]
		}
		for _, r := range o.recs {
			seq := o.pool[r.off : r.off+r.n]
			id := int32(-1)
			for _, cand := range byHash[r.hash] {
				if slices.Equal(c.Chains[cand].Mods, seq) {
					id = cand
					break
				}
			}
			if id < 0 {
				id = int32(len(c.Chains))
				c.Chains = append(c.Chains, Chain{ID: id, Mods: slices.Clone(seq)})
				byHash[r.hash] = append(byHash[r.hash], id)
			}
			ch := &c.Chains[id]
			ch.Files++
			ch.Bytes += r.win
			c.PathChain[r.path] = id
		}
		o.pool, o.recs = nil, nil
	}
	for m := range c.Stats {
		c.Stats[m].Files = t.ModOff[m+1] - t.ModOff[m]
		c.VRAMWinKB += c.Stats[m].VRAMWinKB
	}
	// chain -> files CSR
	c.ChainFileOff = make([]uint32, len(c.Chains)+1)
	for i := range c.Chains {
		c.ChainFileOff[i+1] = c.ChainFileOff[i] + c.Chains[i].Files
		c.ConflictPaths += int(c.Chains[i].Files)
	}
	c.ChainFiles = make([]uint32, c.ChainFileOff[len(c.Chains)])
	fill := slices.Clone(c.ChainFileOff[:len(c.Chains)])
	for p, id := range c.PathChain {
		if id >= 0 {
			c.ChainFiles[fill[id]] = uint32(p)
			fill[id]++
		}
	}
	// W/L/M + mod -> chains CSR + union-find contexts
	parent := make([]int32, nm)
	for i := range parent {
		parent[i] = int32(i)
	}
	var find func(int32) int32
	find = func(x int32) int32 {
		for parent[x] != x {
			parent[x] = parent[parent[x]]
			x = parent[x]
		}
		return x
	}
	c.ModChainOff = make([]uint32, nm+1)
	for _, ch := range c.Chains {
		last := len(ch.Mods) - 1
		for i, m := range ch.Mods {
			st := &c.Stats[m]
			st.Chains++
			c.ModChainOff[m+1]++
			if i == 0 {
				st.Win += ch.Files
			} else {
				st.Lose += ch.Files
				if i < last {
					st.Mid += ch.Files
				}
			}
			if i > 0 {
				a, b := find(int32(ch.Mods[0])), find(int32(m))
				if a != b {
					parent[b] = a
				}
			}
		}
	}
	for i := 1; i <= nm; i++ {
		c.ModChainOff[i] += c.ModChainOff[i-1]
	}
	c.ModChains = make([]int32, c.ModChainOff[nm])
	mfill := slices.Clone(c.ModChainOff[:nm])
	for _, ch := range c.Chains {
		for _, m := range ch.Mods {
			c.ModChains[mfill[m]] = ch.ID
			mfill[m]++
		}
	}
	// contexts
	ctxOf := map[int32]int32{}
	for m := 0; m < nm; m++ {
		st := &c.Stats[m]
		st.Context = -1
		if prio[m] >= 0 && st.Files > 0 && st.Win == 0 && st.Unique == 0 {
			st.Full = true
		}
		if st.Chains == 0 {
			continue
		}
		r := find(int32(m))
		id, ok := ctxOf[r]
		if !ok {
			id = int32(len(c.Contexts))
			ctxOf[r] = id
			c.Contexts = append(c.Contexts, Context{ID: id})
		}
		st.Context = id
		c.Contexts[id].Mods = append(c.Contexts[id].Mods, uint32(m))
	}
	for i := range c.Chains {
		ctx := c.Stats[c.Chains[i].Mods[0]].Context
		c.Chains[i].Context = ctx
		c.Contexts[ctx].Chains = append(c.Contexts[ctx].Chains, int32(i))
		c.Contexts[ctx].Files += c.Chains[i].Files
	}
	for i := range c.Contexts {
		slices.SortFunc(c.Contexts[i].Mods, func(a, b uint32) int { return int(prio[a]) - int(prio[b]) })
	}
	return c
}

// ModChainIDs returns chain ids a mod participates in.
func (c *Conflicts) ModChainIDs(m uint32) []int32 {
	return c.ModChains[c.ModChainOff[m]:c.ModChainOff[m+1]]
}

// ChainPathIDs returns path ids of a chain.
func (c *Conflicts) ChainPathIDs(id int32) []uint32 {
	return c.ChainFiles[c.ChainFileOff[id]:c.ChainFileOff[id+1]]
}
