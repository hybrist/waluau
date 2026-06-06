import { useState, useMemo, useEffect, useRef } from 'react';
import { createPortal } from 'react-dom';

export default function FileSearchModal({ isOpen, onClose, items }) {
  const [search, setSearch] = useState('');
  const [selectedIndex, setSelectedIndex] = useState(0);
  const inputRef = useRef(null);
  const resultsRef = useRef(null);

  useEffect(() => {
    const timer = setTimeout(() => {
      inputRef.current?.focus();
    }, 50);
    return () => clearTimeout(timer);
  }, []);

  const filteredItems = useMemo(() => {
    if (!search.trim()) return items;
    const query = search.toLowerCase();
    return items.filter(item => item.name.toLowerCase().includes(query));
  }, [items, search]);

  useEffect(() => {
    if (!isOpen) return;

    const handleKeyDown = (e) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        e.stopPropagation();
        onClose();
      } else if (e.key === 'ArrowDown') {
        e.preventDefault();
        e.stopPropagation();
        setSelectedIndex(prev => (filteredItems.length > 0 ? (prev + 1) % filteredItems.length : 0));
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        e.stopPropagation();
        setSelectedIndex(prev => (filteredItems.length > 0 ? (prev - 1 + filteredItems.length) % filteredItems.length : 0));
      } else if (e.key === 'Enter') {
        e.preventDefault();
        e.stopPropagation();
        if (filteredItems[selectedIndex]) {
          filteredItems[selectedIndex].onSelect();
          onClose();
        }
      }
    };

    window.addEventListener('keydown', handleKeyDown, true);
    return () => window.removeEventListener('keydown', handleKeyDown, true);
  }, [isOpen, filteredItems, selectedIndex, onClose]);

  useEffect(() => {
    const activeEl = resultsRef.current?.querySelector('.search-item.active');
    if (activeEl) {
      activeEl.scrollIntoView({ block: 'nearest' });
    }
  }, [selectedIndex]);

  if (!isOpen) return null;

  return createPortal(
    <div className="search-modal-backdrop" onClick={onClose}>
      <div className="search-modal-container" onClick={e => e.stopPropagation()}>
        <div className="search-modal-header">
          <svg className="search-input-icon" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5">
            <circle cx="11" cy="11" r="8"></circle>
            <line x1="21" y1="21" x2="16.65" y2="16.65"></line>
          </svg>
          <input
            ref={inputRef}
            type="text"
            className="search-input-field"
            placeholder="Search files by name..."
            value={search}
            onChange={e => {
              setSearch(e.target.value);
              setSelectedIndex(0);
            }}
          />
        </div>
        <div className="search-modal-results" ref={resultsRef}>
          {filteredItems.length === 0 ? (
            <div className="search-no-results">No files found</div>
          ) : (
            filteredItems.map((item, idx) => (
              <div
                key={item.path}
                className={`search-item ${idx === selectedIndex ? 'active' : ''}`}
                onClick={() => {
                  item.onSelect();
                  onClose();
                }}
              >
                <div className="search-item-info">
                  <span className="search-item-name">{item.name}</span>
                  <span className="search-item-path">{item.path.replace('../../../', '')}</span>
                </div>
                <span className={`search-category-tag tag-${item.type}`}>
                  {item.category}
                </span>
              </div>
            ))
          )}
        </div>
        <div className="search-modal-footer">
          <span>&uarr;&darr; to navigate</span>
          <span>&crarr; to select</span>
          <span>esc to close</span>
        </div>
      </div>
    </div>,
    document.body
  );
}
