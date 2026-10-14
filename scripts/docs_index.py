#!/usr/bin/env python3
"""Generate the docs index tables from the files they list. Standard library only.

每个并行 PR 以前都手动往同一张表的末尾追加一行，于是互相冲突（#45）。现在表由
这里生成：PR 只新增自己那个 Markdown 文件，`make docs-index` 把表重新写出来，
`scripts/check.sh` 里的 `--check` 负责拦住忘记生成的改动。
"""
import argparse
from pathlib import Path
import re
import sys

BEGIN = '<!-- docs-index:begin -->'
END = '<!-- docs-index:end -->'
DECISION_FILE = re.compile(r'^(\d{4})-.+\.md$')
STATUS = re.compile(r'^-\s*状态：\s*(\S.*?)\s*$')
LINK = re.compile(r'\[[^\]]*\]\(([^)]+)\)')
FIX = '跑 `make docs-index`（或 `python3 scripts/docs_index.py --write`）重新生成，不要手改表。'


class Invalid(Exception):
    """文档本身写得不对，光生成解决不了，要作者改文件。"""


def heading(rel, lines):
    for index, line in enumerate(lines):
        if line.startswith('# '):
            return line[2:].strip(), lines[index + 1:]
    raise Invalid(f'{rel}：没有一级标题，索引取不到名字')


def cell(rel, value):
    if '|' in value:
        raise Invalid(f'{rel}：“{value}”里有竖线，Markdown 表格放不下，请改掉')
    return value


def decision_rows(directory, rel_dir):
    rows = []
    seen = {}
    for path in sorted(directory.glob('*.md')):
        name = DECISION_FILE.match(path.name)
        if not name:
            continue
        rel = f'{rel_dir}/{path.name}'
        number = name.group(1)
        # 两个会话拿了同一个编号，是索引表盖不住的问题：谁后合谁的记录就被挤出一行。
        if number in seen:
            raise Invalid(f'{rel} 和 {seen[number]} 用了同一个编号 {number}，'
                          f'后写的那条改成下一个没人用的编号')
        seen[number] = rel
        text = path.read_text(encoding='utf-8')
        title, _ = heading(rel, text.splitlines())
        prefix = f'{number}. '
        if not title.startswith(prefix):
            raise Invalid(f'{rel}：一级标题是“{title}”，要以“{prefix}”开头，和文件名的编号一致')
        status = next((STATUS.match(line).group(1) for line in text.splitlines()
                       if STATUS.match(line)), None)
        if status is None:
            raise Invalid(f'{rel}：没有“- 状态：采用”这一行，索引取不到状态')
        rows.append(f'| [{number}]({path.name}) | {cell(rel, title[len(prefix):])} '
                    f'| {cell(rel, status)} |')
    return ['| 编号 | 标题 | 状态 |', '| --- | --- | --- |', *rows]


def subsystem_rows(directory, rel_dir):
    rows = []
    for path in sorted(directory.glob('*.md')):
        if path.name == 'README.md':
            continue
        rel = f'{rel_dir}/{path.name}'
        title, rest = heading(rel, path.read_text(encoding='utf-8').splitlines())
        # 摘要是一级标题后面第一行有内容的文字，必须是 `> 一句话`。
        first = next((line for line in rest if line.strip()), '')
        summary = first[2:].strip() if first.startswith('> ') else ''
        if not summary:
            raise Invalid(f'{rel}：一级标题下面要有一行 `> 一句话`，说明这个子系统做什么；'
                          f'索引第一列用的就是它')
        rows.append(f'| {cell(rel, summary)} | [{cell(rel, title)}]({path.name}) |')
    return ['| 子系统 | 参考 |', '| --- | --- |', *rows]


def span(rel, text):
    """生成区是哪两行：返回 BEGIN 和 END 所在的行号（从 1 起）。

    两份标记（union 合并也可能留下这个）同样拒绝：那时候“生成区”是哪一段说不清，
    `--check` 比对的和读者看到的就会是两回事。
    """
    lines = text.splitlines()
    begin = [number for number, line in enumerate(lines, 1) if line.strip() == BEGIN]
    end = [number for number, line in enumerate(lines, 1) if line.strip() == END]
    if len(begin) != 1 or len(end) != 1 or end[0] < begin[0]:
        raise Invalid(f'{rel}：{BEGIN} 和 {END} 这对标记要各出现一次、按这个顺序，'
                      f'表就是放在它们中间的；现在 begin 有 {len(begin)} 处、'
                      f'end 有 {len(end)} 处，把标记改回一对')
    return begin[0], end[0]


def misplaced(rel, text, table):
    """生成区以外的索引行：`[(行号, 原文), …]`。

    索引只在标记之间。生成区已经列了某份文档，区外再出现一行指向它，读者就看到两份
    索引，而且区外那份不会随文件更新（#80：PR #40 的三行停在 0012 的旧标题上）。

    判断只看两件事：这一行是表格行，而且它要么就是生成出来的表头，要么链到生成区已经
    列出的文档。所以前言、后记、带记录链接的句子、和索引无关的表格都留着。
    """
    begin, end = span(rel, text)
    listed = {target for row in table for target in LINK.findall(row)}
    header = table[0].strip()
    found = []
    for number, line in enumerate(text.splitlines(), 1):
        if begin <= number <= end or not line.lstrip().startswith('|'):
            continue
        if line.strip() == header or listed.intersection(LINK.findall(line)):
            found.append((number, line))
    return found


def splice(rel, text, table):
    """把表放回 README 的标记之间，并删掉区外的索引行；其余正文一字不动。"""
    begin, end = span(rel, text)
    strays = {number for number, _ in misplaced(rel, text, table)}
    kept = []
    for number, line in enumerate(text.splitlines(), 1):
        if number == begin:
            kept.append(line)
            kept += table
        elif begin < number < end or number in strays:
            continue
        else:
            kept.append(line)
    return '\n'.join(kept) + ('\n' if text.endswith('\n') else '')


def current(rel, text):
    begin, end = span(rel, text)
    return [line for line in text.splitlines()[begin:end - 1] if line.strip()]


def difference(rel, want, got):
    """说清楚差在哪：少了哪行、多了哪行，还是只是顺序不对。"""
    missing = [line for line in want if line not in got]
    extra = [line for line in got if line not in want]
    if not missing and not extra:
        return [f'{rel} 的索引表顺序不对（行都在，排列和生成的不一样）。']
    report = [f'{rel} 的索引表和目录对不上。']
    report += [f'  少了：{line}' for line in missing]
    report += [f'  多了（文件已经不在，或内容已经改了）：{line}' for line in extra]
    return report


def indexes(repo):
    decisions = repo / 'docs' / 'decisions'
    subsystems = repo / 'docs' / 'subsystems'
    return [
        (decisions / 'README.md', 'docs/decisions/README.md',
         decision_rows(decisions, 'docs/decisions')),
        (subsystems / 'README.md', 'docs/subsystems/README.md',
         subsystem_rows(subsystems, 'docs/subsystems')),
    ]


def outside(rel, found):
    """区外的索引行：每一行都列出来，作者才知道要删哪几行。"""
    report = [f'{rel} 在生成区外还有索引行，读者会看到两份索引：']
    report += [f'  第 {number} 行：{line.strip()}' for number, line in found]
    return report


def check(repo):
    problems = []
    for path, rel, table in indexes(repo):
        text = path.read_text(encoding='utf-8')
        got = current(rel, text)
        want = [line for line in table if line.strip()]
        if got != want:
            problems += difference(rel, want, got)
        found = misplaced(rel, text, table)
        if found:
            problems += outside(rel, found)
    if problems:
        print('\n'.join(problems))
        print(FIX)
        return 1
    return 0


def write(repo):
    # 先把两张表都算出来再落盘：某份文档写得不对时，不留下半张生成的表。
    spliced = [(path, splice(rel, path.read_text(encoding='utf-8'), table))
               for path, rel, table in indexes(repo)]
    for path, text in spliced:
        path.write_text(text, encoding='utf-8')
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(description='生成或检查 docs 下的索引表')
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument('--check', action='store_true', help='只比对，不写文件；对不上退出 1')
    mode.add_argument('--write', action='store_true', help='把生成的表写回两个 README')
    parser.add_argument('--repo', type=Path, default=Path(__file__).resolve().parents[1],
                        help='仓库根目录，默认是这个脚本所在仓库')
    args = parser.parse_args(argv)
    try:
        return check(args.repo) if args.check else write(args.repo)
    except Invalid as problem:
        # 这类问题重新生成一遍也不会好，所以不提示跑生成命令，直接说文件要怎么改。
        print(problem, file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
