import AppKit
import LauncherCore

private final class BackgroundView: NSView {
    override func draw(_ dirtyRect: NSRect) {
        NSColor.windowBackgroundColor.setFill()
        dirtyRect.fill()
    }
}

@MainActor
private func cellLabel(size: CGFloat, weight: NSFont.Weight = .regular,
                       color: NSColor = .secondaryLabelColor) -> NSTextField {
    let field = NSTextField(labelWithString: "")
    field.translatesAutoresizingMaskIntoConstraints = false
    field.font = .systemFont(ofSize: size, weight: weight)
    field.textColor = color
    field.lineBreakMode = .byTruncatingTail
    field.maximumNumberOfLines = 1
    return field
}

private final class EntryCell: NSTableCellView {
    private let title = cellLabel(size: 13, color: .labelColor)
    private let detail = cellLabel(size: 11)
    private let mark = cellLabel(size: 11, weight: .bold, color: .labelColor)

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        addSubview(title)
        addSubview(detail)
        addSubview(mark)
        textField = title
        title.setContentHuggingPriority(.required, for: .horizontal)
        title.setContentCompressionResistancePriority(.defaultHigh, for: .horizontal)
        detail.alignment = .left
        detail.setContentHuggingPriority(.init(rawValue: 1), for: .horizontal)
        detail.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        mark.alignment = .right
        NSLayoutConstraint.activate([
            title.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 12),
            title.centerYAnchor.constraint(equalTo: centerYAnchor),
            detail.leadingAnchor.constraint(equalTo: title.trailingAnchor, constant: 8),
            detail.firstBaselineAnchor.constraint(equalTo: title.firstBaselineAnchor),
            detail.trailingAnchor.constraint(equalTo: mark.leadingAnchor, constant: -8),
            mark.widthAnchor.constraint(equalToConstant: 24),
            mark.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -12),
            mark.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
    }

    required init?(coder: NSCoder) { fatalError("Use init(frame:)") }

    func configure(with entry: Entry) {
        title.stringValue = entry.title
        detail.stringValue = entry.detail
        mark.stringValue = entry.mark ?? ""
        mark.isHidden = entry.mark == nil
    }
}

public final class LauncherViewController: NSViewController, NSTableViewDataSource,
    NSTableViewDelegate, NSSearchFieldDelegate {
    private let model: LauncherModel
    private let search = NSSearchField()
    private let table = NSTableView()
    private let statusLabel = NSTextField(labelWithString: "")
    private var displayedRows: [Row] = []
    private var updating = false

    public init(model: LauncherModel) {
        self.model = model
        super.init(nibName: nil, bundle: nil)
        model.onChange = { [weak self] in self?.update() }
    }

    required init?(coder: NSCoder) { fatalError("Use init(model:)") }

    public override func loadView() {
        let root = BackgroundView(frame: NSRect(x: 0, y: 0, width: 560, height: 420))
        search.placeholderString = "Search apps and projects"
        search.delegate = self
        search.setAccessibilityLabel("Search apps and projects")

        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("entry"))
        column.resizingMask = .autoresizingMask
        table.addTableColumn(column)
        table.headerView = nil
        table.style = .plain
        table.columnAutoresizingStyle = .lastColumnOnlyAutoresizingStyle
        table.allowsMultipleSelection = false
        table.dataSource = self
        table.delegate = self
        table.target = self
        table.action = #selector(focusSearch)
        table.doubleAction = #selector(activateClickedRow)
        table.setAccessibilityLabel("Apps and projects")

        let scroll = NSScrollView()
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.documentView = table

        statusLabel.font = .systemFont(ofSize: 11)
        statusLabel.textColor = .secondaryLabelColor
        statusLabel.lineBreakMode = .byTruncatingTail
        statusLabel.maximumNumberOfLines = 1
        for subview in [search, scroll, statusLabel] {
            subview.translatesAutoresizingMaskIntoConstraints = false
            root.addSubview(subview)
        }
        NSLayoutConstraint.activate([
            search.topAnchor.constraint(equalTo: root.topAnchor, constant: 12),
            search.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 12),
            search.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -12),
            scroll.topAnchor.constraint(equalTo: search.bottomAnchor, constant: 8),
            scroll.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            scroll.bottomAnchor.constraint(equalTo: statusLabel.topAnchor, constant: -8),
            statusLabel.leadingAnchor.constraint(equalTo: search.leadingAnchor),
            statusLabel.trailingAnchor.constraint(equalTo: search.trailingAnchor),
            statusLabel.bottomAnchor.constraint(equalTo: root.bottomAnchor, constant: -10),
        ])
        view = root
    }

    public override func viewDidLoad() {
        super.viewDidLoad()
        update()
    }

    public override func viewDidLayout() {
        super.viewDidLayout()
        table.sizeLastColumnToFit()
    }

    public override func viewDidAppear() {
        super.viewDidAppear()
        focusSearch()
    }

    @objc private func focusSearch() { view.window?.makeFirstResponder(search) }

    @objc private func activateClickedRow() {
        let index = table.clickedRow
        guard displayedRows.indices.contains(index), case .entry = displayedRows[index] else { return }
        model.select(row: index)
        model.activateSelected()
        focusSearch()
    }

    private func update() {
        guard isViewLoaded else { return }
        updating = true
        defer { updating = false }
        if displayedRows != model.rows {
            displayedRows = model.rows
            table.reloadData()
        }
        if let row = model.selectedRow {
            table.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
            table.scrollRowToVisible(row)
        } else {
            table.deselectAll(nil)
        }
        if search.stringValue != model.query { search.stringValue = model.query }
        statusLabel.stringValue = model.status
    }

    public func layoutForRendering() {
        view.layoutSubtreeIfNeeded()
        table.reloadData()
        update()
        if let scroll = table.enclosingScrollView {
            let count = table.numberOfRows
            let height = count > 0 ? table.rect(ofRow: count - 1).maxY : 0
            table.setFrameSize(NSSize(width: scroll.contentSize.width, height: height))
        }
        table.sizeLastColumnToFit()
        table.tile()
        for row in 0..<table.numberOfRows {
            let rowView = table.rowView(atRow: row, makeIfNecessary: true)
            _ = table.view(atColumn: 0, row: row, makeIfNecessary: true)
            rowView?.isSelected = table.isRowSelected(row)
        }
        table.layoutSubtreeIfNeeded()
        view.layoutSubtreeIfNeeded()
    }

    public func controlTextDidChange(_ notification: Notification) { model.query = search.stringValue }

    public func control(_ control: NSControl, textView: NSTextView,
                        doCommandBy commandSelector: Selector) -> Bool {
        switch commandSelector {
        case #selector(NSResponder.moveDown(_:)): model.moveSelection(1)
        case #selector(NSResponder.moveUp(_:)): model.moveSelection(-1)
        case #selector(NSResponder.insertNewline(_:)): model.activateSelected()
        case #selector(NSResponder.cancelOperation(_:)):
            if model.query.isEmpty { view.window?.performClose(nil) } else { model.query = "" }
        default: return false
        }
        return true
    }

    public func numberOfRows(in tableView: NSTableView) -> Int { displayedRows.count }

    public func tableView(_ tableView: NSTableView, isGroupRow row: Int) -> Bool {
        if case .header = displayedRows[row] { return true }
        return false
    }

    public func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool {
        if case .entry = displayedRows[row] { return true }
        return false
    }

    public func tableViewSelectionDidChange(_ notification: Notification) {
        if !updating && table.selectedRow != model.selectedRow { model.select(row: table.selectedRow) }
    }

    public func tableView(_ tableView: NSTableView, heightOfRow row: Int) -> CGFloat { 24 }

    public func tableView(_ tableView: NSTableView, rowViewForRow row: Int) -> NSTableRowView? {
        let view = NSTableRowView()
        if case .entry(let entry) = displayedRows[row] {
            var label = "\(entry.title), \(entry.detail)"
            if entry.mark?.contains("!") == true { label += ", not answering" }
            if entry.mark?.contains("?") == true { label += ", name used on more than one machine" }
            view.setAccessibilityLabel(label)
        }
        return view
    }

    public func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?,
                          row: Int) -> NSView? {
        switch displayedRows[row] {
        case .header(let text):
            return labelCell(in: tableView, identifier: "header", text: text, weight: .semibold)
        case .note(let text):
            return labelCell(in: tableView, identifier: "note", text: text, weight: .regular)
        case .entry(let entry):
            let id = NSUserInterfaceItemIdentifier("entry")
            let cell = tableView.makeView(withIdentifier: id, owner: self) as? EntryCell
                ?? EntryCell(frame: .zero)
            cell.identifier = id
            cell.configure(with: entry)
            return cell
        }
    }

    private func labelCell(in tableView: NSTableView, identifier: String, text: String,
                           weight: NSFont.Weight) -> NSTableCellView {
        let id = NSUserInterfaceItemIdentifier(identifier)
        if let cell = tableView.makeView(withIdentifier: id, owner: self) as? NSTableCellView {
            cell.textField?.stringValue = text
            return cell
        }
        let cell = NSTableCellView()
        cell.identifier = id
        let label = cellLabel(size: 11, weight: weight)
        label.stringValue = text
        cell.addSubview(label)
        cell.textField = label
        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 12),
            label.trailingAnchor.constraint(lessThanOrEqualTo: cell.trailingAnchor, constant: -12),
            label.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
        ])
        return cell
    }
}
