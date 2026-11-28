import sys, logging
sys.path.insert(0, sys.argv[1])
logging.basicConfig(level=logging.WARNING)
import ia2standins as S
from ia2standins import Node, IA2
w = S.World(1234)
link = Node(0x1E, name="docs", content=["docs"])
p = Node(IA2.IA2_ROLE_PARAGRAPH, content=["Read the ", link, " first."])
p.attach(w)
t = p.com.QueryInterface(IA2.IAccessibleText)
print("text", repr(t.text(0, -2)))
print("attrs", t.attributes(0))
ht = p.com.QueryInterface(IA2.IAccessibleHypertext)
print("nlinks", ht.nHyperlinks)
print(S.CALLS)
