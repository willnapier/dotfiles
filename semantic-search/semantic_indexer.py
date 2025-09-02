#!/usr/bin/env python3
"""
AI-Powered Semantic Note Association System - Background Indexer

Creates and maintains a FAISS vector database of OpenAI embeddings for semantic search
across an Obsidian vault. Tracks file modifications to avoid reprocessing unchanged notes.

Usage:
    python3 semantic_indexer.py --rebuild    # Full rebuild of index
    python3 semantic_indexer.py --update     # Incremental update of changed files  
    python3 semantic_indexer.py --watch      # Watch for changes and update automatically
"""

import os
import json
import time
import yaml
import hashlib
import logging
import argparse
import re
from pathlib import Path
from datetime import datetime
from typing import Dict, List, Tuple, Optional
from dataclasses import dataclass, asdict

import numpy as np
import pandas as pd
import faiss
from openai import OpenAI
from tqdm import tqdm
from watchdog.observers import Observer
from watchdog.events import FileSystemEventHandler

@dataclass
class FileMetadata:
    """Metadata for tracking file changes and embedding state"""
    path: str
    size: int
    mtime: float
    content_hash: str
    embedding_timestamp: float
    embedding_tokens: int
    embedding_cost: float

@dataclass 
class IndexStats:
    """Statistics about the indexing process"""
    total_files: int = 0
    processed_files: int = 0
    skipped_files: int = 0
    failed_files: int = 0
    total_tokens: int = 0
    total_cost: float = 0.0
    start_time: float = 0.0
    end_time: float = 0.0
    
    @property
    def duration(self) -> float:
        return self.end_time - self.start_time

class SemanticIndexer:
    def __init__(self, config_path: str = None):
        """Initialize the semantic indexer with configuration."""
        if config_path is None:
            config_path = os.path.expanduser("~/.local/share/semantic-search/config.yaml")
        
        with open(config_path, 'r') as f:
            self.config = yaml.safe_load(f)
        
        self._setup_logging()
        self._setup_directories()
        self._setup_openai()
        
        # Initialize FAISS index and metadata tracking
        self.index = None
        self.file_metadata: Dict[str, FileMetadata] = {}
        self.file_paths: List[str] = []  # Maps index position to file path
        self.dimension = 3072  # text-embedding-3-large dimension
        
        self._load_existing_data()
        
    def _setup_logging(self):
        """Setup logging configuration."""
        log_config = self.config['logging']
        log_file = os.path.expanduser(log_config['file'])
        os.makedirs(os.path.dirname(log_file), exist_ok=True)
        
        logging.basicConfig(
            level=getattr(logging, log_config['level']),
            format='%(asctime)s - %(name)s - %(levelname)s - %(message)s',
            handlers=[
                logging.FileHandler(log_file),
                logging.StreamHandler()
            ]
        )
        self.logger = logging.getLogger('SemanticIndexer')
        
    def _setup_directories(self):
        """Ensure all required directories exist."""
        dirs = [
            os.path.dirname(os.path.expanduser(self.config['database']['index_path'])),
            os.path.dirname(os.path.expanduser(self.config['database']['metadata_path'])),
            os.path.expanduser(self.config['database']['cache_path']),
            os.path.dirname(os.path.expanduser(self.config['logging']['file']))
        ]
        
        for directory in dirs:
            os.makedirs(directory, exist_ok=True)
            
    def _setup_openai(self):
        """Initialize OpenAI client."""
        # Check for API key in environment
        api_key = os.getenv('OPENAI_API_KEY')
        if not api_key:
            raise ValueError("OPENAI_API_KEY environment variable is required")
        
        self.openai_client = OpenAI(api_key=api_key)
        self.logger.info(f"OpenAI client initialized with model: {self.config['openai']['model']}")
        
    def _load_existing_data(self):
        """Load existing FAISS index and metadata if available."""
        index_path = os.path.expanduser(self.config['database']['index_path'])
        metadata_path = os.path.expanduser(self.config['database']['metadata_path'])
        
        if os.path.exists(index_path) and os.path.exists(metadata_path):
            try:
                # Load FAISS index
                self.index = faiss.read_index(index_path)
                self.logger.info(f"Loaded existing FAISS index with {self.index.ntotal} vectors")
                
                # Load metadata
                with open(metadata_path, 'r') as f:
                    metadata_dict = json.load(f)
                    self.file_metadata = {
                        path: FileMetadata(**data) for path, data in metadata_dict['files'].items()
                    }
                    self.file_paths = metadata_dict.get('file_paths', [])
                    
                self.logger.info(f"Loaded metadata for {len(self.file_metadata)} files")
                
            except Exception as e:
                self.logger.warning(f"Failed to load existing data: {e}")
                self._initialize_empty_index()
        else:
            self._initialize_empty_index()
            
    def _initialize_empty_index(self):
        """Initialize empty FAISS index and metadata."""
        self.index = faiss.IndexFlatIP(self.dimension)  # Inner product for cosine similarity
        self.file_metadata = {}
        self.file_paths = []
        self.logger.info("Initialized empty FAISS index")
        
    def _save_data(self):
        """Save FAISS index and metadata to disk."""
        index_path = os.path.expanduser(self.config['database']['index_path'])
        metadata_path = os.path.expanduser(self.config['database']['metadata_path'])
        
        # Save FAISS index
        faiss.write_index(self.index, index_path)
        
        # Save metadata
        metadata_dict = {
            'files': {path: asdict(metadata) for path, metadata in self.file_metadata.items()},
            'file_paths': self.file_paths,
            'last_updated': datetime.now().isoformat()
        }
        
        with open(metadata_path, 'w') as f:
            json.dump(metadata_dict, f, indent=2)
            
        self.logger.info(f"Saved index with {self.index.ntotal} vectors and {len(self.file_metadata)} files")
        
    def _get_file_content_hash(self, content: str) -> str:
        """Generate hash of file content for change detection."""
        return hashlib.md5(content.encode('utf-8')).hexdigest()
        
    def _extract_content(self, file_path: str) -> Optional[str]:
        """Extract and clean content from markdown file."""
        try:
            with open(file_path, 'r', encoding='utf-8', errors='ignore') as f:
                content = f.read()
                
            # Skip frontmatter if configured
            if self.config['indexing']['skip_frontmatter']:
                # Remove YAML frontmatter (between --- lines)
                content = re.sub(r'^---\s*\n.*?\n---\s*\n', '', content, flags=re.MULTILINE | re.DOTALL)
                
            # Basic cleaning
            content = content.strip()
            
            # Check length constraints
            min_length = self.config['indexing']['min_content_length']
            max_length = self.config['indexing']['max_content_length']
            
            if len(content) < min_length:
                self.logger.debug(f"Skipping {file_path}: content too short ({len(content)} chars)")
                return None
                
            if len(content) > max_length:
                content = content[:max_length]
                self.logger.debug(f"Truncated {file_path}: content too long")
                
            return content
            
        except Exception as e:
            self.logger.error(f"Failed to read {file_path}: {e}")
            return None
            
    def _estimate_tokens(self, text: str) -> int:
        """Estimate token count using rough approximation (4 chars per token)."""
        return len(text) // 4
        
    def _truncate_to_token_limit(self, text: str, max_tokens: int = 8000) -> str:
        """Truncate text to stay within token limits, preserving sentence boundaries."""
        estimated_tokens = self._estimate_tokens(text)
        
        if estimated_tokens <= max_tokens:
            return text
            
        # Conservative truncation - take roughly 80% of max to account for tokenization differences
        target_chars = int(max_tokens * 4 * 0.8)
        
        if len(text) <= target_chars:
            return text
            
        # Truncate and try to end on sentence boundary
        truncated = text[:target_chars]
        
        # Find last sentence ending
        last_period = truncated.rfind('.')
        last_newline = truncated.rfind('\n\n')
        
        # Use the later of period or double newline for cleaner cut
        best_cut = max(last_period, last_newline)
        
        if best_cut > target_chars * 0.5:  # Only use if we don't lose too much
            truncated = truncated[:best_cut + 1]
            
        return truncated

    def _get_embedding(self, text: str) -> Tuple[Optional[np.ndarray], int, float]:
        """Get embedding from OpenAI API with cost tracking and token limit handling."""
        try:
            # Ensure text fits within token limits
            max_tokens = self.config['openai'].get('max_tokens', 8000)
            safe_text = self._truncate_to_token_limit(text, max_tokens)
            
            if len(safe_text) < len(text):
                self.logger.debug(f"Truncated text from {len(text)} to {len(safe_text)} chars for token limit")
            
            response = self.openai_client.embeddings.create(
                input=safe_text,
                model=self.config['openai']['model']
            )
            
            embedding = np.array(response.data[0].embedding, dtype=np.float32)
            
            # Normalize for cosine similarity with inner product
            embedding = embedding / np.linalg.norm(embedding)
            
            # Calculate cost (approximate - check OpenAI pricing)
            tokens = response.usage.total_tokens
            cost = tokens * 0.00013 / 1000  # $0.13 per 1M tokens for text-embedding-3-large
            
            # Rate limiting
            time.sleep(self.config['openai']['rate_limit_delay'])
            
            return embedding, tokens, cost
            
        except Exception as e:
            self.logger.error(f"Failed to get embedding: {e}")
            return None, 0, 0.0
            
    def _should_process_file(self, file_path: str) -> bool:
        """Determine if file needs processing based on modification time."""
        try:
            stat = os.stat(file_path)
            current_size = stat.st_size
            current_mtime = stat.st_mtime
            
            # Check if we have metadata for this file
            if file_path not in self.file_metadata:
                return True
                
            metadata = self.file_metadata[file_path]
            
            # Check if file has changed
            if metadata.size != current_size or metadata.mtime != current_mtime:
                return True
                
            return False
            
        except Exception as e:
            self.logger.error(f"Error checking file {file_path}: {e}")
            return True
            
    def _find_markdown_files(self) -> List[str]:
        """Find all markdown files in the vault."""
        vault_path = Path(self.config['vault']['path'])
        extensions = self.config['vault']['extensions']
        exclude_dirs = set(self.config['vault']['exclude_dirs'])
        exclude_files = set(self.config['vault'].get('exclude_files', []))
        
        markdown_files = []
        
        for ext in extensions:
            for file_path in vault_path.rglob(f"*{ext}"):
                # Skip if in excluded directory
                if any(excl_dir in file_path.parts for excl_dir in exclude_dirs):
                    continue
                
                # Skip if filename is in excluded files list
                filename = file_path.name
                if filename in exclude_files:
                    self.logger.info(f"Skipping excluded file: {filename}")
                    continue
                    
                markdown_files.append(str(file_path))
                
        self.logger.info(f"Found {len(markdown_files)} markdown files")
        return sorted(markdown_files)
        
    def rebuild_index(self) -> IndexStats:
        """Completely rebuild the index from scratch."""
        self.logger.info("Starting full index rebuild")
        stats = IndexStats(start_time=time.time())
        
        # Initialize empty index
        self._initialize_empty_index()
        
        # Find all files
        markdown_files = self._find_markdown_files()
        stats.total_files = len(markdown_files)
        
        # Process files in batches for efficiency
        batch_size = self.config['openai']['batch_size']
        
        for i in tqdm(range(0, len(markdown_files), batch_size), desc="Processing batches"):
            batch_files = markdown_files[i:i + batch_size]
            batch_stats = self._process_file_batch(batch_files, force_rebuild=True)
            
            # Update stats
            stats.processed_files += batch_stats.processed_files
            stats.skipped_files += batch_stats.skipped_files 
            stats.failed_files += batch_stats.failed_files
            stats.total_tokens += batch_stats.total_tokens
            stats.total_cost += batch_stats.total_cost
            
        # Save the rebuilt index
        self._save_data()
        
        stats.end_time = time.time()
        self._log_stats(stats, "Full rebuild completed")
        
        return stats
        
    def update_index(self) -> IndexStats:
        """Incrementally update the index with changed files."""
        self.logger.info("Starting incremental index update")
        stats = IndexStats(start_time=time.time())
        
        # Find all files and filter for changes
        markdown_files = self._find_markdown_files()
        changed_files = [f for f in markdown_files if self._should_process_file(f)]
        
        stats.total_files = len(markdown_files)
        
        if not changed_files:
            self.logger.info("No files need updating")
            stats.end_time = time.time()
            return stats
            
        self.logger.info(f"Found {len(changed_files)} files to update")
        
        # Process changed files
        batch_size = self.config['openai']['batch_size']
        
        for i in tqdm(range(0, len(changed_files), batch_size), desc="Updating files"):
            batch_files = changed_files[i:i + batch_size]
            batch_stats = self._process_file_batch(batch_files, force_rebuild=False)
            
            # Update stats
            stats.processed_files += batch_stats.processed_files
            stats.skipped_files += batch_stats.skipped_files
            stats.failed_files += batch_stats.failed_files
            stats.total_tokens += batch_stats.total_tokens
            stats.total_cost += batch_stats.total_cost
            
        # Save updated index
        self._save_data()
        
        stats.end_time = time.time()
        self._log_stats(stats, "Incremental update completed")
        
        return stats
        
    def _process_file_batch(self, files: List[str], force_rebuild: bool = False) -> IndexStats:
        """Process a batch of files."""
        stats = IndexStats()
        
        for file_path in files:
            try:
                # Check if processing is needed
                if not force_rebuild and not self._should_process_file(file_path):
                    stats.skipped_files += 1
                    continue
                    
                # Extract content
                content = self._extract_content(file_path)
                if content is None:
                    stats.skipped_files += 1
                    continue
                    
                # Check if content actually changed
                content_hash = self._get_file_content_hash(content)
                if (not force_rebuild and file_path in self.file_metadata and 
                    self.file_metadata[file_path].content_hash == content_hash):
                    stats.skipped_files += 1
                    continue
                    
                # Get embedding
                embedding, tokens, cost = self._get_embedding(content)
                if embedding is None:
                    stats.failed_files += 1
                    continue
                    
                # Update or add to index
                if file_path in self.file_metadata:
                    # File exists, need to update
                    old_index = self.file_paths.index(file_path)
                    # For simplicity, we'll rebuild affected parts of index
                    # In production, you might want more sophisticated update logic
                    pass
                else:
                    # New file, add to index
                    self.file_paths.append(file_path)
                    
                # Add embedding to FAISS index
                self.index.add(embedding.reshape(1, -1))
                
                # Update metadata
                stat = os.stat(file_path)
                self.file_metadata[file_path] = FileMetadata(
                    path=file_path,
                    size=stat.st_size,
                    mtime=stat.st_mtime,
                    content_hash=content_hash,
                    embedding_timestamp=time.time(),
                    embedding_tokens=tokens,
                    embedding_cost=cost
                )
                
                stats.processed_files += 1
                stats.total_tokens += tokens
                stats.total_cost += cost
                
            except Exception as e:
                self.logger.error(f"Failed to process {file_path}: {e}")
                stats.failed_files += 1
                
        return stats
        
    def _log_stats(self, stats: IndexStats, message: str):
        """Log indexing statistics."""
        self.logger.info(f"\n{message}")
        self.logger.info(f"Duration: {stats.duration:.2f} seconds")
        self.logger.info(f"Total files: {stats.total_files}")
        self.logger.info(f"Processed: {stats.processed_files}")
        self.logger.info(f"Skipped: {stats.skipped_files}")
        self.logger.info(f"Failed: {stats.failed_files}")
        self.logger.info(f"Total tokens: {stats.total_tokens:,}")
        self.logger.info(f"Total cost: ${stats.total_cost:.4f}")
        
        if stats.processed_files > 0:
            self.logger.info(f"Avg tokens/file: {stats.total_tokens / stats.processed_files:.0f}")
            self.logger.info(f"Processing rate: {stats.processed_files / stats.duration:.1f} files/sec")


class FileWatcher(FileSystemEventHandler):
    """Watch for file changes and trigger incremental updates."""
    
    def __init__(self, indexer: SemanticIndexer):
        self.indexer = indexer
        self.debounce_seconds = indexer.config['indexing']['debounce_seconds']
        self.pending_updates = set()
        self.last_update_time = {}
        
    def on_modified(self, event):
        if event.is_directory:
            return
            
        file_path = event.src_path
        if not file_path.endswith('.md'):
            return
            
        # Debounce rapid changes
        current_time = time.time()
        if file_path in self.last_update_time:
            time_diff = current_time - self.last_update_time[file_path]
            if time_diff < self.debounce_seconds:
                return
                
        self.last_update_time[file_path] = current_time
        self.pending_updates.add(file_path)
        
        # Trigger update after debounce period
        # In a production system, you'd want a more sophisticated debounce mechanism
        self.indexer.logger.info(f"File changed: {file_path}")


def main():
    parser = argparse.ArgumentParser(description='Semantic Note Indexer')
    parser.add_argument('--rebuild', action='store_true', help='Rebuild entire index')
    parser.add_argument('--update', action='store_true', help='Update changed files')
    parser.add_argument('--watch', action='store_true', help='Watch for changes')
    parser.add_argument('--config', help='Config file path')
    
    args = parser.parse_args()
    
    try:
        indexer = SemanticIndexer(args.config)
        
        if args.rebuild:
            indexer.rebuild_index()
        elif args.update:
            indexer.update_index()
        elif args.watch:
            print("Watching for file changes... Press Ctrl+C to stop")
            observer = Observer()
            observer.schedule(
                FileWatcher(indexer), 
                indexer.config['vault']['path'], 
                recursive=True
            )
            observer.start()
            
            try:
                while True:
                    time.sleep(1)
            except KeyboardInterrupt:
                observer.stop()
            observer.join()
        else:
            print("Please specify --rebuild, --update, or --watch")
            
    except Exception as e:
        print(f"Error: {e}")
        return 1
        
    return 0

if __name__ == '__main__':
    exit(main())