import {describe,it,expect} from 'vitest';
import {readFileSync} from 'node:fs';
import {experimentalMetadata} from '../src/experimentalNodes';
import {libraryCategories,metadata,parseGraph} from '../src/model';
describe('experimental node library',()=>{
 it('offers a separate experimental category without replacing existing kinds',()=>{expect(libraryCategories).toContain('実験');expect(metadata['text.romaji-to-hiragana'].title).toContain('実用');for(const [kind,item]of Object.entries(experimentalMetadata)){expect(kind).toMatch(/^experimental\./);expect(item.category).toBe('実験');expect(item.title.length).toBeGreaterThan(0);}});
 it('loads the experimental text sample and names every new kind',()=>{const graph=parseGraph(readFileSync(new URL('../../examples/graphs/experimental-text.json',import.meta.url),'utf8'));for(const node of graph.nodes)expect(metadata[node.kind]).toBeDefined();expect(graph.nodes.some(n=>n.kind==='experimental.text-trim')).toBe(true);});
});
