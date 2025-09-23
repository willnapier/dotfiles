#!/usr/bin/env python3
"""
Semantic Query Interface

Performs fast semantic similarity searches against the indexed note database.

Usage:
    python3 semantic_query.py --text "decision making under uncertainty"
    python3 semantic_query.py --file "/path/to/note.md"
    python3 semantic_query.py --file "/path/to/note.md" --limit 5
"""

import os
import json
import yaml
import argparse
import logging
from pathlib import Path
from typing import List, Tuple, Optional
from dataclasses import dataclass

import numpy as np
import faiss
from openai import OpenAI

@dataclass
class SearchResult:
    """A single search result with similarity and metadata."""
    file_path: str
    similarity: float
    title: str = ""
    snippet: str = ""
    
    @property
    def filename(self) -> str:
        return Path(self.file_path).stem
        
    @property
    def relative_path(self) -> str:
        """Path relative to vault root."""
        try:
            # Use environment variable or home directory for cross-platform compatibility
            vault_root = os.environ.get('FORGE', os.path.expanduser('~/Forge'))
            if not vault_root.endswith('/'):
                vault_root += '/'
            return os.path.relpath(self.file_path, vault_root)
        except:
            return self.file_path

class SemanticQuery:
    def __init__(self, config_path: str = None):
        """Initialize semantic query interface."""
        if config_path is None:
            config_path = os.path.expanduser("~/.local/share/semantic-search/config.yaml")
            
        with open(config_path, 'r') as f:
            self.config = yaml.safe_load(f)
            
        self._setup_logging()
        self._setup_openai()
        self._load_index()
        
    def _setup_logging(self):
        """Setup logging configuration."""
        logging.basicConfig(level=logging.WARNING)  # Keep quiet for query mode
        self.logger = logging.getLogger('SemanticQuery')
        
    def _setup_openai(self):
        """Initialize OpenAI client if API key is available."""
        api_key = os.getenv('OPENAI_API_KEY')
        if not api_key:
            self.logger.warning("OPENAI_API_KEY not set - semantic search will be unavailable")
            self.openai_client = None
        else:
            self.openai_client = OpenAI(api_key=api_key)
        
    def _load_index(self):
        """Load FAISS index and metadata."""
        index_path = os.path.expanduser(self.config['database']['index_path'])
        metadata_path = os.path.expanduser(self.config['database']['metadata_path'])
        
        if not os.path.exists(index_path) or not os.path.exists(metadata_path):
            raise FileNotFoundError(
                "Index not found. Run 'semantic_indexer.py --rebuild' first."
            )
            
        # Load FAISS index
        self.index = faiss.read_index(index_path)
        
        # Load metadata
        with open(metadata_path, 'r') as f:
            metadata_dict = json.load(f)
            self.file_paths = metadata_dict['file_paths']
            
        self.logger.info(f"Loaded index with {self.index.ntotal} vectors")
        
    def _get_embedding(self, text: str) -> Optional[np.ndarray]:
        """Get embedding for query text."""
        if self.openai_client is None:
            print("❌ Semantic search unavailable: OpenAI API key not configured")
            print("💡 To enable semantic search, set your OpenAI API key:")
            print("   export OPENAI_API_KEY='your-api-key-here'")
            return None
            
        try:
            response = self.openai_client.embeddings.create(
                input=text,
                model=self.config['openai']['model']
            )
            
            embedding = np.array(response.data[0].embedding, dtype=np.float32)
            
            # Normalize for cosine similarity
            embedding = embedding / np.linalg.norm(embedding)
            
            return embedding
            
        except Exception as e:
            self.logger.error(f"Failed to get embedding: {e}")
            return None
            
    def _extract_file_content(self, file_path: str) -> str:
        """Extract content from a file for embedding."""
        try:
            with open(file_path, 'r', encoding='utf-8', errors='ignore') as f:
                content = f.read()
                
            # Remove frontmatter like the indexer does
            import re
            content = re.sub(r'^---\s*\n.*?\n---\s*\n', '', content, flags=re.MULTILINE | re.DOTALL)
            
            return content.strip()
            
        except Exception as e:
            self.logger.error(f"Failed to read {file_path}: {e}")
            return ""
            
    def _get_file_title(self, file_path: str) -> str:
        """Extract title from file (first heading or filename)."""
        try:
            with open(file_path, 'r', encoding='utf-8', errors='ignore') as f:
                lines = f.readlines()
                
            # Look for first markdown heading
            for line in lines[:10]:  # Check first 10 lines
                line = line.strip()
                if line.startswith('#'):
                    return line.lstrip('#').strip()
                    
            # Fallback to filename
            return Path(file_path).stem
            
        except:
            return Path(file_path).stem
            
    def _get_file_snippet(self, file_path: str, query_text: str = "") -> str:
        """Extract a relevant snippet from the file."""
        try:
            content = self._extract_file_content(file_path)
            
            if not content:
                return ""
                
            # Simple snippet extraction - first paragraph or first 200 chars
            paragraphs = [p.strip() for p in content.split('\n\n') if p.strip()]
            
            if paragraphs:
                snippet = paragraphs[0]
                if len(snippet) > 200:
                    snippet = snippet[:200] + "..."
                return snippet
            else:
                content = content.replace('\n', ' ')
                if len(content) > 200:
                    return content[:200] + "..."
                return content
                
        except:
            return ""
            
    def search_by_text(self, query_text: str, limit: int = None) -> List[SearchResult]:
        """Search for notes similar to given text query."""
        if limit is None:
            limit = self.config['query']['max_results']
            
        # Get query embedding
        query_embedding = self._get_embedding(query_text)
        if query_embedding is None:
            return []
            
        # Search FAISS index
        similarities, indices = self.index.search(
            query_embedding.reshape(1, -1), 
            min(limit, self.index.ntotal)
        )
        
        # Build results
        results = []
        threshold = self.config['query']['similarity_threshold']
        
        for similarity, idx in zip(similarities[0], indices[0]):
            if similarity < threshold:
                continue
                
            file_path = self.file_paths[idx]
            
            result = SearchResult(
                file_path=file_path,
                similarity=float(similarity),
                title=self._get_file_title(file_path),
                snippet=self._get_file_snippet(file_path, query_text)
            )
            
            results.append(result)
            
        return results
        
    def search_by_file(self, file_path: str, limit: int = None) -> List[SearchResult]:
        """Search for notes similar to given file."""
        content = self._extract_file_content(file_path)
        if not content:
            return []
            
        return self.search_by_text(content, limit)
        
    def format_results(self, results: List[SearchResult], query_desc: str) -> str:
        """Format search results for display."""
        if not results:
            return f"No results found for: {query_desc}"
            
        output = []
        output.append(f"Results for: {query_desc}")
        output.append("─" * 80)
        
        for result in results:
            output.append(f"{result.similarity:.2f}  {result.filename}")
            
            if self.config['query']['result_format'] == 'detailed':
                if result.snippet:
                    snippet_lines = result.snippet.replace('\n', ' ')[:100]
                    output.append(f"      {snippet_lines}")
                output.append("")  # Empty line between detailed results
                
        return "\n".join(output)

def main():
    parser = argparse.ArgumentParser(description='Semantic Note Search')
    parser.add_argument('--text', help='Search by text query')
    parser.add_argument('--file', help='Search by file similarity')
    parser.add_argument('--limit', type=int, help='Maximum number of results')
    parser.add_argument('--config', help='Config file path')
    
    args = parser.parse_args()
    
    if not args.text and not args.file:
        print("Please specify either --text or --file")
        return 1
        
    try:
        query = SemanticQuery(args.config)
        
        if args.text:
            results = query.search_by_text(args.text, args.limit)
            query_desc = f'"{args.text}"'
        else:
            results = query.search_by_file(args.file, args.limit)
            query_desc = f'file: {os.path.basename(args.file)}'
            
        print(query.format_results(results, query_desc))
        
    except Exception as e:
        print(f"Error: {e}")
        return 1
        
    return 0

if __name__ == '__main__':
    exit(main())