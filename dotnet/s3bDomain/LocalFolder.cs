using System;
using System.Collections.Generic;
using System.Text;
using System.Reflection;

namespace s3b
{
    public class LocalFolder : Model
    {
        public bool recurse;
       // public List<LocalFile> files;
        public BackupSet backupSet;

        public List<BackupLog> backupLogs = new List<BackupLog>();

        public enum stages
        {
            archiveStage = 1,
            compressStage = 2,
            encryptStage = 4,
            uploadStage = 8,
            cleanStage = 16
        }


        public int getStageCode()
        {
            int stageCode = 0;

            if (stage == "new")
            {
                stageCode |= (int)stages.archiveStage | (int)stages.compressStage | (int)stages.encryptStage | (int)stages.uploadStage | (int)stages.cleanStage;
            }

            else if (stage == "archive")
            {
                stageCode |= (int)stages.archiveStage | (int)stages.compressStage | (int)stages.encryptStage | (int)stages.uploadStage | (int)stages.cleanStage;
            }

            else if (stage == "compress")
            {
                stageCode |= (int)stages.compressStage | (int)stages.encryptStage | (int)stages.uploadStage | (int)stages.cleanStage;
            }

            else if (stage == "encrypt")
            {
                stageCode |=  (int)stages.encryptStage | (int)stages.uploadStage | (int)stages.cleanStage;
            }

            else if (stage == "upload")
            {
                stageCode |= (int)stages.uploadStage | (int)stages.cleanStage ;
            }

            else if (stage == "clean")
            {
                stageCode |= (int)stages.cleanStage;
            }

            return stageCode;
        }
                

        public LocalFolder()
        {
            tableName = "local_folder";

            last_error = string.Empty;

            recurse = true;

            //files = new List<LocalFile>();

            encrypted_file_name = "";
            encrypted_file_size = 0;
            upload_datetime = new DateTime(1970, 1, 1);

            
            clearDirty();
        }

        public long id
        {
            get
            {
                return Convert.ToInt64(getPropValue(MethodBase.GetCurrentMethod().Name));
            }
            set
            {
                setPropValue(MethodBase.GetCurrentMethod().Name, value);
            }
        }

        public long backup_set_id
        {
            get
            {
                return Convert.ToInt64(getPropValue(MethodBase.GetCurrentMethod().Name));
            }
            set
            {
                setPropValue(MethodBase.GetCurrentMethod().Name, value);
            }
        }


        public string folder_path
        {
            get
            {
                return getPropValue(MethodBase.GetCurrentMethod().Name).ToString();
            }
            set
            {
                setPropValue(MethodBase.GetCurrentMethod().Name, value);
            }
        }

        public string stage
        {
            get
            {
                return getPropValue(MethodBase.GetCurrentMethod().Name).ToString();
            }
            set
            {
                setPropValue(MethodBase.GetCurrentMethod().Name, value);
            }
        }

        public string status
        {
            get
            {
                return getPropValue(MethodBase.GetCurrentMethod().Name).ToString();
            }
            set
            {
                setPropValue(MethodBase.GetCurrentMethod().Name, value);
            }
        }

        public string last_error
        {
            get
            {
                return getPropValue(MethodBase.GetCurrentMethod().Name).ToString();
            }
            set
            {
                setPropValue(MethodBase.GetCurrentMethod().Name, value);
            }
        }

        public string encrypted_file_name
        {
            get
            {
                return getPropValue(MethodBase.GetCurrentMethod().Name).ToString();
            }
            set
            {
                setPropValue(MethodBase.GetCurrentMethod().Name, value);
            }
        }

        public long encrypted_file_size
        {
            get
            {
                return Convert.ToInt64(getPropValue(MethodBase.GetCurrentMethod().Name));
            }
            set
            {
                setPropValue(MethodBase.GetCurrentMethod().Name, value);
            }
        }

        public DateTime upload_datetime
        {
            get
            {
                return Convert.ToDateTime(getPropValue(MethodBase.GetCurrentMethod().Name));
            }
            set
            {
                setPropValue(MethodBase.GetCurrentMethod().Name, value);
            }
        }

public string getArchiveName()
        {
            StringBuilder archiveName = new StringBuilder(System.Environment.MachineName);
            archiveName.Append("_");
            archiveName.Append(System.Environment.UserName);
            archiveName.Append("_");
            archiveName.Append(folder_path);
            archiveName.Replace('\\', '_');
            archiveName.Replace('/', '_');
            archiveName.Replace(":", "");
            archiveName.Replace(" ", "_");

            return archiveName.ToString();
        }
    }
}
