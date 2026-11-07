import os
from app import github_integration as g
g.API_BASE=os.environ['GH_BASE']
from app.main import app
